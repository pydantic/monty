//! [`Input`] for a JSON document streamed from a `Jiter`: pydantic-core's
//! JSON-mode coercions (`input/input_json.rs`) applied token by token, so a
//! document costs its output and never an intermediate tree.
//!
//! One parser, in a `RefCell`, serves every input of a document. An input is
//! the token the parser stood on plus a copy of the parser at that point
//! (`Jiter::clone` is a few words; it shares the data and starts an empty
//! tape). A validator consumes its token through `scalar`, or by iterating a
//! container; a validator that then needs the whole input back (an error
//! report, an `Any`) rewinds to the copy and reads the token again. A union
//! member runs from a checkpoint that `attempt` restores on failure, and a
//! container item a failed member left unread is skipped when the item is
//! finished, so the parser always moves on to the next item.

use std::cell::RefCell;

use jiter::{Jiter, JiterError, JiterErrorType, JsonErrorType, NumberAny, NumberInt, Peek};
use monty_types::ValidationInputType;

use crate::{
    bytecode::VM,
    heap::HeapId,
    modules::{
        json::load::{JsonLoadError, materialize_json_value},
        pydantic::{
            catalogue::ErrorType,
            coerce::{
                bigint_value, bytes_as_date, bytes_as_datetime, bytes_as_time, bytes_as_timedelta, float_as_datetime,
                float_as_duration, float_as_int, float_as_time, int_as_bool, int_as_datetime, int_as_duration,
                int_as_time, str_as_bool, str_as_float, str_as_int,
            },
            errors::{ValError, ValResult},
            input::{Input, InputKind, Mapping, Match, SeqKind, Sequence},
        },
    },
    types::{
        allocate_string,
        bytes::allocate_bytes,
        decimal::{self as decimal_type, py as py_decimal},
    },
    value::Value,
};

/// The parser a document's inputs share.
pub(crate) type JsonParser<'j> = RefCell<Jiter<'j>>;

/// The nesting pydantic's JSON parser allows: a non-empty container this
/// deep is `recursion limit exceeded`, reported at its first item as `jiter`
/// reports it.
const RECURSION_LIMIT: usize = 200;

/// One JSON value under validation.
pub(crate) enum JsonInput<'j, 'a> {
    /// A token of the document.
    Token {
        parser: &'a JsonParser<'j>,
        peek: Peek,
        /// The parser at the token's first byte, to read the token again.
        start: Jiter<'j>,
        /// Containers enclosing the token.
        depth: usize,
    },
    /// An object key, already read as text; `strict` does not apply to keys.
    Key(String),
}

/// A consumed scalar token, or the text of a key.
enum Scalar {
    Null,
    Bool(bool),
    Int(NumberInt),
    Float(f64),
    Str(String),
}

impl<'j, 'a> JsonInput<'j, 'a> {
    /// The document's root: the first token of `parser`.
    pub(crate) fn root(parser: &'a JsonParser<'j>) -> Result<Self, JiterError> {
        Self::at(parser, 0)
    }

    /// Consumes whatever of the document is left: the root itself if no
    /// validator read it, then the trailing input, which must be empty.
    pub(crate) fn finish(&mut self) -> Result<(), JiterError> {
        self.skip_unread()?;
        match self {
            Self::Token { parser, .. } => parser.borrow_mut().finish(),
            Self::Key(_) => Ok(()),
        }
    }

    /// The token the parser stands on, `depth` containers down.
    fn at(parser: &'a JsonParser<'j>, depth: usize) -> Result<Self, JiterError> {
        let mut jiter = parser.borrow_mut();
        let peek = jiter.peek()?;
        Ok(Self::Token {
            parser,
            peek,
            start: jiter.clone(),
            depth,
        })
    }

    /// Whether the token has been read past.
    fn consumed(&self) -> bool {
        match self {
            Self::Token { parser, start, .. } => parser.borrow().current_index() != start.current_index(),
            Self::Key(_) => true,
        }
    }

    /// Puts the parser back on the token's first byte.
    fn rewind(&self) {
        if let Self::Token { parser, start, .. } = self {
            *parser.borrow_mut() = start.clone();
        }
    }

    /// Skips the token if no validator consumed it.
    fn skip_unread(&mut self) -> Result<(), JiterError> {
        if self.consumed() {
            return Ok(());
        }
        match self {
            Self::Token { parser, peek, .. } => parser.borrow_mut().known_skip(*peek),
            Self::Key(_) => Ok(()),
        }
    }

    /// Consumes a scalar token, or reads a key; `None` leaves an array or
    /// object where it is. Reading starts from the token's first byte, so a
    /// fallback can read a token another coercion consumed.
    fn scalar(&mut self) -> ValResult<Option<Scalar>> {
        self.rewind();
        match self {
            Self::Key(key) => Ok(Some(Scalar::Str(key.clone()))),
            Self::Token { parser, peek, .. } => {
                let mut jiter = parser.borrow_mut();
                let scalar = match *peek {
                    Peek::Null => {
                        jiter.known_null().map_err(ValError::Json)?;
                        Scalar::Null
                    }
                    Peek::True | Peek::False => Scalar::Bool(jiter.known_bool(*peek).map_err(ValError::Json)?),
                    Peek::String => Scalar::Str(jiter.known_str().map_err(ValError::Json)?.to_owned()),
                    Peek::Array | Peek::Object => return Ok(None),
                    _ => match jiter.known_number(*peek).map_err(ValError::Json)? {
                        NumberAny::Int(int) => Scalar::Int(int),
                        NumberAny::Float(float) => Scalar::Float(float),
                    },
                };
                Ok(Some(scalar))
            }
        }
    }

    /// `strict` as it applies to this input: never to a key, which is text
    /// whatever the schema says.
    fn strict(&self, strict: bool) -> bool {
        strict && matches!(self, Self::Token { .. })
    }

    /// A coercion's result as a validation result: the error reads the
    /// token again as its input.
    fn outcome<T>(&mut self, result: Result<Match<T>, ErrorType>, vm: &mut VM<'_>) -> ValResult<Match<T>> {
        match result {
            Ok(matched) => Ok(matched),
            Err(error_type) => self.error(error_type, vm),
        }
    }

    /// A token as a sequence or mapping cursor over its items, or
    /// `error_type` for any other token.
    fn container<C>(
        &mut self,
        wanted: Peek,
        error_type: ErrorType,
        make: impl FnOnce(&'a JsonParser<'j>, usize) -> C,
        vm: &mut VM<'_>,
    ) -> ValResult<C> {
        self.rewind();
        match self {
            Self::Token {
                parser, peek, depth, ..
            } if *peek == wanted => Ok(make(parser, *depth + 1)),
            _ => self.error(error_type, vm),
        }
    }

    /// The first item of a container at the recursion limit is where
    /// `jiter` gives up on the document.
    fn check_depth(&self) -> Result<(), JiterError> {
        match self {
            Self::Token { start, depth, .. } if *depth > RECURSION_LIMIT => Err(JiterError {
                error_type: JiterErrorType::JsonError(JsonErrorType::RecursionLimitExceeded),
                index: start.current_index(),
            }),
            _ => Ok(()),
        }
    }
}

/// A loader failure as a validation failure.
fn load_failure(err: JsonLoadError) -> ValError {
    match err {
        JsonLoadError::Parse(err) => ValError::Json(err),
        JsonLoadError::Run(err) => ValError::Run(err),
    }
}

impl<'j, 'a> Input for JsonInput<'j, 'a> {
    type Seq = JsonSequence<'j, 'a>;
    type Map = JsonMapping<'j, 'a>;

    const INPUT_TYPE: ValidationInputType = ValidationInputType::Json;

    /// The token as a value, read from its first byte: the parser is left
    /// after the token however much of it a validator had consumed.
    fn to_value(&mut self, vm: &mut VM<'_>) -> ValResult<Value> {
        self.rewind();
        match self {
            Self::Token {
                parser, peek, depth, ..
            } => materialize_json_value(*peek, &mut parser.borrow_mut(), *depth, vm).map_err(load_failure),
            Self::Key(key) => Ok(allocate_string(key.clone(), vm.heap)),
        }
    }

    fn is_none(&self) -> bool {
        matches!(self, Self::Token { peek: Peek::Null, .. })
    }

    fn validate_bool(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<bool>> {
        let strict = self.strict(strict);
        let result = match self.scalar()? {
            Some(Scalar::Bool(b)) => Ok(Match::exact(b)),
            Some(Scalar::Str(text)) if !strict => str_as_bool(&text).map(Match::lax),
            Some(Scalar::Int(NumberInt::Int(int))) if !strict => int_as_bool(int).map(Match::lax),
            Some(Scalar::Float(float)) if !strict => match float_as_int(float) {
                Ok(int) => int_as_bool(int).map(Match::lax),
                Err(_) => Err(ErrorType::BoolType {}),
            },
            _ => Err(ErrorType::BoolType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_int(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let strict = self.strict(strict);
        let result = match self.scalar()? {
            Some(Scalar::Int(NumberInt::Int(int))) => Ok(Match::exact(Value::Int(int))),
            Some(Scalar::Int(NumberInt::BigInt(big))) => Ok(Match::exact(bigint_value(big, vm.heap))),
            Some(Scalar::Bool(b)) if !strict => Ok(Match::lax(Value::Int(i64::from(b)))),
            Some(Scalar::Float(float)) if !strict => float_as_int(float).map(|int| Match::lax(Value::Int(int))),
            Some(Scalar::Str(text)) if !strict => str_as_int(&text, vm.heap).map(Match::lax),
            _ => Err(ErrorType::IntType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_float(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<f64>> {
        let strict = self.strict(strict);
        let result = match self.scalar()? {
            Some(Scalar::Float(float)) => Ok(Match::exact(float)),
            Some(Scalar::Int(int)) => Ok(Match::strict(f64::from(int))),
            Some(Scalar::Bool(b)) if !strict => Ok(Match::lax(if b { 1.0 } else { 0.0 })),
            Some(Scalar::Str(text)) if !strict => str_as_float(&text).map(Match::lax),
            _ => Err(ErrorType::FloatType {}),
        };
        self.outcome(result, vm)
    }

    /// A JSON string is a strict, not exact, match: in JSON text is also how
    /// dates and the like are written, so `str` must not win a union outright.
    fn validate_str(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let result = match self.scalar()? {
            Some(Scalar::Str(text)) => Ok(Match::strict(allocate_string(text, vm.heap))),
            _ => Err(ErrorType::StringType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_bytes(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let result = match self.scalar()? {
            Some(Scalar::Str(text)) => Ok(Match::strict(allocate_bytes(text.into_bytes(), vm.heap))),
            _ => Err(ErrorType::BytesType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_datetime(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let strict = self.strict(strict);
        let result = match self.scalar()? {
            Some(Scalar::Str(text)) => bytes_as_datetime(text.as_bytes(), vm).map(Match::strict),
            Some(Scalar::Int(NumberInt::Int(int))) if !strict => int_as_datetime(int, 0, vm).map(Match::lax),
            Some(Scalar::Float(float)) if !strict => float_as_datetime(float, vm).map(Match::lax),
            _ => Err(ErrorType::DatetimeType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_date(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let result = match self.scalar()? {
            Some(Scalar::Str(text)) => bytes_as_date(text.as_bytes(), vm.heap).map(Match::strict),
            _ => Err(ErrorType::DateType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_time(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let strict = self.strict(strict);
        let result = match self.scalar()? {
            Some(Scalar::Str(text)) => bytes_as_time(text.as_bytes(), vm).map(Match::strict),
            Some(Scalar::Int(NumberInt::Int(int))) if !strict => int_as_time(int, 0, vm).map(Match::lax),
            Some(Scalar::Float(float)) if !strict => float_as_time(float, vm).map(Match::lax),
            _ => Err(ErrorType::TimeType {}),
        };
        self.outcome(result, vm)
    }

    fn validate_timedelta(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let strict = self.strict(strict);
        let result = match self.scalar()? {
            Some(Scalar::Str(text)) => bytes_as_timedelta(text.as_bytes(), vm.heap).map(Match::strict),
            Some(Scalar::Int(NumberInt::Int(int))) if !strict => int_as_duration(int, vm.heap).map(Match::lax),
            Some(Scalar::Float(float)) if !strict => float_as_duration(float, vm.heap).map(Match::lax),
            _ => Err(ErrorType::TimeDeltaType {}),
        };
        self.outcome(result, vm)
    }

    /// Text and numbers parse as decimals; a float goes through its shortest
    /// text, as pydantic-core's `f64::to_string` writes it.
    fn validate_decimal(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>> {
        let text = match self.scalar()? {
            Some(Scalar::Str(text)) => text,
            Some(Scalar::Int(NumberInt::Int(int))) => int.to_string(),
            Some(Scalar::Int(NumberInt::BigInt(big))) => big.to_string(),
            Some(Scalar::Float(float)) => float.to_string(),
            _ => return self.error(ErrorType::DecimalType {}, vm),
        };
        let parsed = decimal_type::parse::parse_str(&text).map_err(ValError::Run)?;
        let result = parsed
            .map(|decimal| Match::strict(py_decimal::decimal_value(decimal, vm)))
            .map_err(|_| ErrorType::DecimalParsing {});
        self.outcome(result, vm)
    }

    fn validate_sequence(
        &mut self,
        kind: SeqKind,
        _strict: bool,
        vm: &mut VM<'_>,
    ) -> ValResult<Match<JsonSequence<'j, 'a>>> {
        let error_type = match kind {
            SeqKind::List => ErrorType::ListType {},
            SeqKind::Tuple => ErrorType::TupleType {},
            SeqKind::Set => ErrorType::SetType {},
            SeqKind::FrozenSet => ErrorType::FrozenSetType {},
        };
        let seq = self.container(Peek::Array, error_type, JsonSequence::new, vm)?;
        Ok(if kind == SeqKind::List {
            Match::exact(seq)
        } else {
            Match::strict(seq)
        })
    }

    fn validate_dict(&mut self, _strict: bool, vm: &mut VM<'_>) -> ValResult<Match<JsonMapping<'j, 'a>>> {
        self.container(Peek::Object, ErrorType::DictType {}, JsonMapping::new, vm)
            .map(Match::exact)
    }

    fn validate_dataclass_args(&mut self, class_name: &str, vm: &mut VM<'_>) -> ValResult<JsonMapping<'j, 'a>> {
        let error_type = ErrorType::DataclassType {
            class_name: class_name.to_owned(),
        };
        self.container(Peek::Object, error_type, JsonMapping::new, vm)
    }

    fn instance_of(&self, _class_id: HeapId, _vm: &VM<'_>) -> Option<Value> {
        None
    }

    /// Runs `attempt` from the token's first byte, where a failure leaves the
    /// parser, so each union member reads the same tokens.
    fn attempt<R>(
        &mut self,
        vm: &mut VM<'_>,
        attempt: impl FnOnce(&mut Self, &mut VM<'_>) -> ValResult<R>,
    ) -> ValResult<R> {
        self.rewind();
        let result = attempt(self, vm);
        if result.is_err() {
            self.rewind();
        }
        result
    }

    fn kind(&self) -> InputKind {
        match self {
            Self::Key(_) => InputKind::Str,
            Self::Token { peek, start, .. } => match *peek {
                Peek::Null => InputKind::Null,
                Peek::True | Peek::False => InputKind::Bool,
                Peek::String => InputKind::Str,
                Peek::Array => InputKind::Array,
                Peek::Object => InputKind::Object,
                // A number is read on a copy of the parser to tell the two kinds apart.
                _ => match start.clone().known_number(*peek) {
                    Ok(NumberAny::Int(_)) => InputKind::Int,
                    Ok(NumberAny::Float(_)) => InputKind::Float,
                    Err(_) => InputKind::Other,
                },
            },
        }
    }

    fn is_mapping(&self, _vm: &VM<'_>) -> bool {
        matches!(self, Self::Token { peek: Peek::Object, .. })
    }

    fn heap_id(&self) -> Option<HeapId> {
        None
    }
}

/// The items of a JSON array, each a token the parser is stepped to.
pub(crate) struct JsonSequence<'j, 'a> {
    parser: &'a JsonParser<'j>,
    /// The items' depth.
    depth: usize,
    started: bool,
    /// A parse failure while skipping an unread item, reported by the next step.
    pending: Option<JiterError>,
}

impl<'j, 'a> JsonSequence<'j, 'a> {
    fn new(parser: &'a JsonParser<'j>, depth: usize) -> Self {
        Self {
            parser,
            depth,
            started: false,
            pending: None,
        }
    }
}

impl<'j, 'a> Sequence<JsonInput<'j, 'a>> for JsonSequence<'j, 'a> {
    fn len(&self) -> Option<usize> {
        None
    }

    fn next(&mut self, _vm: &mut VM<'_>) -> ValResult<Option<JsonInput<'j, 'a>>> {
        if let Some(err) = self.pending.take() {
            return Err(ValError::Json(err));
        }
        let first = !self.started;
        self.started = true;
        let step = {
            let mut jiter = self.parser.borrow_mut();
            if first { jiter.known_array() } else { jiter.array_step() }
        };
        match step.map_err(ValError::Json)? {
            Some(_) => {
                let item = JsonInput::at(self.parser, self.depth).map_err(ValError::Json)?;
                if first {
                    item.check_depth().map_err(ValError::Json)?;
                }
                Ok(Some(item))
            }
            None => Ok(None),
        }
    }

    fn finish_item(&mut self, mut item: JsonInput<'j, 'a>, _vm: &mut VM<'_>) {
        if let Err(err) = item.skip_unread() {
            self.pending = Some(err);
        }
    }
}

/// The entries of a JSON object, keys read as text.
pub(crate) struct JsonMapping<'j, 'a> {
    parser: &'a JsonParser<'j>,
    /// The values' depth.
    depth: usize,
    started: bool,
    /// A parse failure while skipping an unread value, reported by the next step.
    pending: Option<JiterError>,
}

impl<'j, 'a> JsonMapping<'j, 'a> {
    fn new(parser: &'a JsonParser<'j>, depth: usize) -> Self {
        Self {
            parser,
            depth,
            started: false,
            pending: None,
        }
    }
}

impl<'j, 'a> Mapping<JsonInput<'j, 'a>> for JsonMapping<'j, 'a> {
    fn next(&mut self, _vm: &mut VM<'_>) -> ValResult<Option<(JsonInput<'j, 'a>, JsonInput<'j, 'a>)>> {
        if let Some(err) = self.pending.take() {
            return Err(ValError::Json(err));
        }
        let first = !self.started;
        self.started = true;
        let key = {
            let mut jiter = self.parser.borrow_mut();
            let key = if first { jiter.known_object() } else { jiter.next_key() };
            key.map(|key| key.map(ToOwned::to_owned))
        };
        match key.map_err(ValError::Json)? {
            Some(key) => {
                let value = JsonInput::at(self.parser, self.depth).map_err(ValError::Json)?;
                if first {
                    value.check_depth().map_err(ValError::Json)?;
                }
                Ok(Some((JsonInput::Key(key), value)))
            }
            None => Ok(None),
        }
    }

    fn finish_entry(&mut self, _key: JsonInput<'j, 'a>, mut value: JsonInput<'j, 'a>, _vm: &mut VM<'_>) {
        if let Err(err) = value.skip_unread() {
            self.pending = Some(err);
        }
    }
}
