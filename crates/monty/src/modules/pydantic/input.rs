//! The input a validator reads: a Python value or a streamed JSON document.
//!
//! The [`Input`] trait is what pydantic-core's `Input` trait is for Monty:
//! each method is one coercion the validators need (`validate_int`, a
//! sequence to iterate, a mapping to walk), implemented once for Python
//! values and once for JSON tokens, so the validators in `validate.rs` are
//! written once. Results carry an [`Exactness`], which smart unions use to
//! rank their candidates.

use monty_types::ValidationInputType;

use crate::{
    bytecode::VM,
    heap::HeapId,
    modules::pydantic::{
        arena::ValidatorId,
        catalogue::ErrorType,
        errors::{ValError, ValResult},
    },
    value::Value,
};

/// How closely an input matched its validator, from pydantic-core: exact
/// (`1` for `int`), strict (`True` for `float`, an instance of a subclass) or
/// lax (`'1'` for `int`). A smart union prefers the most exact success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Exactness {
    Lax,
    Strict,
    Exact,
}

/// A coerced value with the exactness of its match.
pub(crate) struct Match<T> {
    pub(crate) value: T,
    pub(crate) exactness: Exactness,
}

impl<T> Match<T> {
    pub(crate) fn exact(value: T) -> Self {
        Self {
            value,
            exactness: Exactness::Exact,
        }
    }

    pub(crate) fn strict(value: T) -> Self {
        Self {
            value,
            exactness: Exactness::Strict,
        }
    }

    pub(crate) fn lax(value: T) -> Self {
        Self {
            value,
            exactness: Exactness::Lax,
        }
    }

    /// Records the match's exactness on `state` and yields the value.
    pub(crate) fn unpack(self, state: &mut State) -> T {
        state.floor_exactness(self.exactness);
        self.value
    }
}

/// Per-validation settings and the bookkeeping unions and the recursion
/// guard need.
pub(crate) struct State {
    /// `strict=` from the call, overriding the lax defaults.
    pub(crate) strict: bool,
    /// The running exactness, tracked only inside a union.
    pub(crate) exactness: Option<Exactness>,
    /// How many dataclass fields the input set, a union tie-breaker.
    pub(crate) fields_set_count: Option<usize>,
    /// The `(object, validator)` pairs on the current descent, so a value
    /// that contains itself is reported rather than walked forever.
    pub(crate) recursion: Vec<(HeapId, ValidatorId)>,
    /// Validator calls so far, for polling the time limit every few calls.
    pub(crate) steps: usize,
}

impl State {
    pub(crate) fn new(strict: bool) -> Self {
        Self {
            strict,
            exactness: None,
            fields_set_count: None,
            recursion: Vec::new(),
            steps: 0,
        }
    }

    /// Lowers the running exactness to `exactness` when inside a union.
    pub(crate) fn floor_exactness(&mut self, exactness: Exactness) {
        match self.exactness {
            None | Some(Exactness::Lax) => {}
            Some(Exactness::Strict) => {
                if exactness == Exactness::Lax {
                    self.exactness = Some(Exactness::Lax);
                }
            }
            Some(Exactness::Exact) => self.exactness = Some(exactness),
        }
    }

    pub(crate) fn add_fields_set(&mut self, count: usize) {
        *self.fields_set_count.get_or_insert(0) += count;
    }
}

/// Which container a sequence input is validated into; the lax rules differ
/// only in which input type counts as exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeqKind {
    List,
    Tuple,
    Set,
    FrozenSet,
}

/// One value under validation, in either input mode.
pub(crate) trait Input: Sized {
    /// Items of a sequence input, each an `Input` of the same mode.
    type Seq: Sequence<Self>;
    /// Entries of a mapping input: keys are strings for a JSON object and
    /// values for a Python dict, so each is an `Input` too.
    type Map: Mapping<Self>;

    const INPUT_TYPE: ValidationInputType;

    /// The input as a Python value, for `Any`, error reports and literal
    /// comparisons. A JSON input is consumed by this.
    fn to_value(&mut self, vm: &mut VM<'_>) -> ValResult<Value>;

    fn is_none(&self) -> bool;

    fn validate_bool(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<bool>>;

    /// An `int` value: `Value::Int` or a big integer.
    fn validate_int(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    fn validate_float(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<f64>>;

    /// A `str` value.
    fn validate_str(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    /// A `bytes` value.
    fn validate_bytes(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    fn validate_datetime(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    fn validate_date(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    fn validate_time(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    fn validate_timedelta(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    fn validate_decimal(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Value>>;

    /// The input as a sequence of `kind`, or the type error for `kind`.
    fn validate_sequence(&mut self, kind: SeqKind, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Self::Seq>>;

    /// The input as a mapping, or `dict_type`.
    fn validate_dict(&mut self, strict: bool, vm: &mut VM<'_>) -> ValResult<Match<Self::Map>>;

    /// The input as the arguments of dataclass `class_name`: a mapping, or
    /// `dataclass_type` naming the class.
    fn validate_dataclass_args(&mut self, class_name: &str, vm: &mut VM<'_>) -> ValResult<Self::Map>;

    /// The input when it is already an instance of the class `class_id`,
    /// which a dataclass validator passes through; never for JSON.
    fn instance_of(&self, class_id: HeapId, vm: &VM<'_>) -> Option<Value>;

    /// Runs `attempt` on this input such that a failure leaves the input
    /// where it was: a JSON input forks its parser and adopts the fork only
    /// on success. Unions try each member through this.
    fn attempt<R>(
        &mut self,
        vm: &mut VM<'_>,
        attempt: impl FnOnce(&mut Self, &mut VM<'_>) -> ValResult<R>,
    ) -> ValResult<R>;

    /// A coarse kind for pruning union members before they run.
    fn kind(&self) -> InputKind;

    /// Whether the input is a mapping, which a namedtuple reads as keyword arguments.
    fn is_mapping(&self, vm: &VM<'_>) -> bool;

    /// The heap object the input is, for the recursion guard; `None` for JSON
    /// and for immediate values.
    fn heap_id(&self) -> Option<HeapId>;

    /// The error `error_type` on this input, consuming a JSON input.
    fn error<T>(&mut self, error_type: ErrorType, vm: &mut VM<'_>) -> ValResult<T> {
        let value = self.to_value(vm)?;
        Err(ValError::owned(error_type, value))
    }
}

/// The shape of an input before coercion, for union pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    Null,
    Bool,
    Int,
    Float,
    Str,
    Array,
    Object,
    Other,
}

/// The items of a sequence input, in order.
pub(crate) trait Sequence<I: Input>: Sized {
    /// The item count when known up front.
    fn len(&self) -> Option<usize>;

    /// The next item, or `None` at the end. Each item must be released with
    /// [`finish_item`](Self::finish_item) once validated.
    fn next(&mut self, vm: &mut VM<'_>) -> ValResult<Option<I>>;

    /// Releases an item: a Python item drops its reference, a JSON item must
    /// have been consumed.
    fn finish_item(&mut self, item: I, vm: &mut VM<'_>);
}

/// The entries of a mapping input, in order.
pub(crate) trait Mapping<I: Input>: Sized {
    /// The next `(key, value)` entry, or `None` at the end.
    fn next(&mut self, vm: &mut VM<'_>) -> ValResult<Option<(I, I)>>;

    fn finish_entry(&mut self, key: I, value: I, vm: &mut VM<'_>);
}
