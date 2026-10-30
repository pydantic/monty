//! The pydantic-core error catalogue: every `ValidationError` line-error type,
//! its `type` string, its Python and JSON message templates and the context
//! fields the templates take.
//!
//! Copied from pydantic-core's `errors/types.rs` so messages match byte for
//! byte. A variant holds exactly the fields its template names; `context()`
//! reports them in declaration order, which is the order `errors()` shows.

use monty_types::{MontyObject, ValidationInputType};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    modules::pydantic::snapshot::snapshot_value,
    value::Value,
};

/// Declares [`ErrorType`]: each variant with its `type` string and context fields.
///
/// Generates the name lookup `from_exception_data` needs, the `type` string,
/// the CamelCase name pydantic puts in context errors, and `context()`.
macro_rules! error_types {
    ($( $variant:ident $name:literal { $($field:ident : $ty:ty),* } ),+ $(,)?) => {
        /// One line error's kind and template parameters.
        #[derive(Debug, Clone, PartialEq)]
        pub(crate) enum ErrorType {
            $( $variant { $($field: $ty),* } ),+
        }

        impl ErrorType {
            /// The `type` string, e.g. `int_parsing`.
            pub(crate) fn type_str(&self) -> &'static str {
                match self {
                    $( Self::$variant { .. } => $name ),+
                }
            }

            /// Builds the type named `name` from the `(key, value)` entries of a
            /// context dict, as `ValidationError.from_exception_data` does.
            pub(crate) fn from_name(name: &str, ctx: &[(String, Value)], vm: &mut VM<'_>) -> RunResult<Self> {
                match name {
                    $( $name => Ok(Self::$variant {
                        $($field: <$ty as CtxField>::from_context(ctx, stringify!($field), stringify!($variant), vm)?),*
                    }), )+
                    other => Err(SimpleException::new_msg(ExcType::KeyError, format!("Invalid error type: '{other}'")).into()),
                }
            }

            /// The template parameters as `errors()` reports them under `ctx`.
            pub(crate) fn context(&self) -> Vec<(&'static str, MontyObject)> {
                match self {
                    $( Self::$variant { $($field),* } => vec![$((stringify!($field), $field.to_context())),*] ),+
                }
            }
        }
    };
}

error_types! {
    NoSuchAttribute "no_such_attribute" { attribute: String },
    JsonInvalid "json_invalid" { error: String },
    JsonType "json_type" {},
    NeedsPythonObject "needs_python_object" { method_name: String },
    RecursionLoop "recursion_loop" {},
    Missing "missing" {},
    FrozenField "frozen_field" {},
    FrozenInstance "frozen_instance" {},
    ExtraForbidden "extra_forbidden" {},
    InvalidKey "invalid_key" {},
    GetAttributeError "get_attribute_error" { error: String },
    ModelType "model_type" { class_name: String },
    ModelAttributesType "model_attributes_type" {},
    DataclassType "dataclass_type" { class_name: String },
    DataclassExactType "dataclass_exact_type" { class_name: String },
    NamedTupleType "named_tuple_type" { class_name: String },
    DefaultFactoryNotCalled "default_factory_not_called" {},
    NoneRequired "none_required" {},
    GreaterThan "greater_than" { gt: Number },
    GreaterThanEqual "greater_than_equal" { ge: Number },
    LessThan "less_than" { lt: Number },
    LessThanEqual "less_than_equal" { le: Number },
    MultipleOf "multiple_of" { multiple_of: Number },
    FiniteNumber "finite_number" {},
    TooShort "too_short" { field_type: String, min_length: usize, actual_length: usize },
    TooLong "too_long" { field_type: String, max_length: usize, actual_length: Option<usize> },
    IterableType "iterable_type" {},
    IterationError "iteration_error" { error: String },
    StringType "string_type" {},
    StringUnicode "string_unicode" {},
    StringTooShort "string_too_short" { min_length: usize },
    StringTooLong "string_too_long" { max_length: usize },
    StringPatternMismatch "string_pattern_mismatch" { pattern: String },
    StringNotAscii "string_not_ascii" {},
    Enum "enum" { expected: String },
    DictType "dict_type" {},
    FrozenDictType "frozen_dict_type" {},
    OrderedDictType "ordered_dict_type" {},
    CounterType "counter_type" {},
    MappingType "mapping_type" { error: String },
    ListType "list_type" {},
    DequeType "deque_type" {},
    TupleType "tuple_type" {},
    SetType "set_type" {},
    SetItemNotHashable "set_item_not_hashable" {},
    BoolType "bool_type" {},
    BoolParsing "bool_parsing" {},
    IntType "int_type" {},
    IntParsing "int_parsing" {},
    IntParsingSize "int_parsing_size" {},
    IntFromFloat "int_from_float" {},
    FloatType "float_type" {},
    FloatParsing "float_parsing" {},
    BytesType "bytes_type" {},
    BytesTooShort "bytes_too_short" { min_length: usize },
    BytesTooLong "bytes_too_long" { max_length: usize },
    BytesInvalidEncoding "bytes_invalid_encoding" { encoding: String, encoding_error: String },
    ValueError "value_error" { error: String },
    AssertionError "assertion_error" { error: String },
    LiteralError "literal_error" { expected: String },
    MissingSentinelError "missing_sentinel_error" {},
    EllipsisError "ellipsis_error" {},
    DateType "date_type" {},
    DateParsing "date_parsing" { error: String },
    DateFromDatetimeParsing "date_from_datetime_parsing" { error: String },
    DateFromDatetimeInexact "date_from_datetime_inexact" {},
    DatePast "date_past" {},
    DateFuture "date_future" {},
    TimeType "time_type" {},
    TimeParsing "time_parsing" { error: String },
    DatetimeType "datetime_type" {},
    DatetimeParsing "datetime_parsing" { error: String },
    DatetimeObjectInvalid "datetime_object_invalid" { error: String },
    DatetimeFromDateParsing "datetime_from_date_parsing" { error: String },
    DatetimePast "datetime_past" {},
    DatetimeFuture "datetime_future" {},
    TimezoneNaive "timezone_naive" {},
    TimezoneAware "timezone_aware" {},
    TimezoneOffset "timezone_offset" { tz_expected: i64, tz_actual: i64 },
    TimeDeltaType "time_delta_type" {},
    TimeDeltaParsing "time_delta_parsing" { error: String },
    FrozenSetType "frozen_set_type" {},
    IsInstanceOf "is_instance_of" { class: String },
    IsSubclassOf "is_subclass_of" { class: String },
    CallableType "callable_type" {},
    UnionTagInvalid "union_tag_invalid" { discriminator: String, tag: String, expected_tags: String },
    UnionTagNotFound "union_tag_not_found" { discriminator: String },
    ArgumentsType "arguments_type" {},
    MissingArgument "missing_argument" {},
    UnexpectedKeywordArgument "unexpected_keyword_argument" {},
    MissingKeywordOnlyArgument "missing_keyword_only_argument" {},
    UnexpectedPositionalArgument "unexpected_positional_argument" {},
    MissingPositionalOnlyArgument "missing_positional_only_argument" {},
    MultipleArgumentValues "multiple_argument_values" {},
    UrlType "url_type" {},
    UrlParsing "url_parsing" { error: String },
    UrlSyntaxViolation "url_syntax_violation" { error: String },
    UrlTooLong "url_too_long" { max_length: usize },
    UrlScheme "url_scheme" { expected_schemes: String },
    UuidType "uuid_type" {},
    UuidParsing "uuid_parsing" { error: String },
    UuidVersion "uuid_version" { expected_version: usize },
    DecimalType "decimal_type" {},
    DecimalParsing "decimal_parsing" {},
    DecimalMaxDigits "decimal_max_digits" { max_digits: u64 },
    DecimalMaxPlaces "decimal_max_places" { decimal_places: u64 },
    DecimalWholeDigits "decimal_whole_digits" { whole_digits: u64 },
    FractionType "fraction_type" {},
    FractionParsing "fraction_parsing" {},
    ComplexType "complex_type" {},
    ComplexStrParsing "complex_str_parsing" {},
}

impl ErrorType {
    /// The message with the Python wording, before substitution.
    fn message_template_python(&self) -> &'static str {
        match self {
            Self::NoSuchAttribute { .. } => "Object has no attribute '{attribute}'",
            Self::JsonInvalid { .. } => "Invalid JSON: {error}",
            Self::JsonType { .. } => "JSON input should be string, bytes or bytearray",
            Self::NeedsPythonObject { .. } => {
                "Cannot check `{method_name}` when validating from json, use a JsonOrPython validator instead"
            }
            Self::RecursionLoop { .. } => "Recursion error - cyclic reference detected",
            Self::Missing { .. } => "Field required",
            Self::FrozenField { .. } => "Field is frozen",
            Self::FrozenInstance { .. } => "Instance is frozen",
            Self::ExtraForbidden { .. } => "Extra inputs are not permitted",
            Self::InvalidKey { .. } => "Keys should be strings",
            Self::GetAttributeError { .. } => "Error extracting attribute: {error}",
            Self::ModelType { .. } => "Input should be a valid dictionary or instance of {class_name}",
            Self::ModelAttributesType { .. } => "Input should be a valid dictionary or object to extract fields from",
            Self::DataclassType { .. } => "Input should be a dictionary or an instance of {class_name}",
            Self::DataclassExactType { .. } => "Input should be an instance of {class_name}",
            Self::NamedTupleType { .. } => "Input should be a tuple, list, dictionary or an instance of {class_name}",
            Self::DefaultFactoryNotCalled { .. } => {
                "The default factory uses validated data, but at least one validation error occurred"
            }
            Self::NoneRequired { .. } => "Input should be None",
            Self::GreaterThan { .. } => "Input should be greater than {gt}",
            Self::GreaterThanEqual { .. } => "Input should be greater than or equal to {ge}",
            Self::LessThan { .. } => "Input should be less than {lt}",
            Self::LessThanEqual { .. } => "Input should be less than or equal to {le}",
            Self::MultipleOf { .. } => "Input should be a multiple of {multiple_of}",
            Self::FiniteNumber { .. } => "Input should be a finite number",
            Self::TooShort { .. } => {
                "{field_type} should have at least {min_length} item{expected_plural} after validation, not {actual_length}"
            }
            Self::TooLong { .. } => {
                "{field_type} should have at most {max_length} item{expected_plural} after validation, not {actual_length}"
            }
            Self::IterableType { .. } => "Input should be iterable",
            Self::IterationError { .. } => "Error iterating over object, error: {error}",
            Self::StringType { .. } => "Input should be a valid string",
            Self::StringUnicode { .. } => {
                "Input should be a valid string, unable to parse raw data as a unicode string"
            }
            Self::StringTooShort { .. } => "String should have at least {min_length} character{expected_plural}",
            Self::StringTooLong { .. } => "String should have at most {max_length} character{expected_plural}",
            Self::StringPatternMismatch { .. } => "String should match pattern '{pattern}'",
            Self::StringNotAscii { .. } => "String should contain only ASCII characters",
            Self::Enum { .. } => "Input should be {expected}",
            Self::DictType { .. } => "Input should be a valid dictionary",
            Self::FrozenDictType { .. } => "Input should be a valid frozendict",
            Self::OrderedDictType { .. } => "Input should be a valid OrderedDict",
            Self::CounterType { .. } => "Input should be a valid Counter",
            Self::MappingType { .. } => "Input should be a valid mapping, error: {error}",
            Self::ListType { .. } => "Input should be a valid list",
            Self::DequeType { .. } => "Input should be a valid deque",
            Self::TupleType { .. } => "Input should be a valid tuple",
            Self::SetType { .. } => "Input should be a valid set",
            Self::SetItemNotHashable { .. } => "Set items should be hashable",
            Self::BoolType { .. } => "Input should be a valid boolean",
            Self::BoolParsing { .. } => "Input should be a valid boolean, unable to interpret input",
            Self::IntType { .. } => "Input should be a valid integer",
            Self::IntParsing { .. } => "Input should be a valid integer, unable to parse string as an integer",
            Self::IntParsingSize { .. } => "Unable to parse input string as an integer, exceeded maximum size",
            Self::IntFromFloat { .. } => "Input should be a valid integer, got a number with a fractional part",
            Self::FloatType { .. } => "Input should be a valid number",
            Self::FloatParsing { .. } => "Input should be a valid number, unable to parse string as a number",
            Self::BytesType { .. } => "Input should be a valid bytes",
            Self::BytesTooShort { .. } => "Data should have at least {min_length} byte{expected_plural}",
            Self::BytesTooLong { .. } => "Data should have at most {max_length} byte{expected_plural}",
            Self::BytesInvalidEncoding { .. } => "Data should be valid {encoding}: {encoding_error}",
            Self::ValueError { .. } => "Value error, {error}",
            Self::AssertionError { .. } => "Assertion failed, {error}",
            Self::LiteralError { .. } => "Input should be {expected}",
            Self::MissingSentinelError { .. } => "Input should be the 'MISSING' sentinel",
            Self::EllipsisError { .. } => "Input should be the 'Ellipsis' literal",
            Self::DateType { .. } => "Input should be a valid date",
            Self::DateParsing { .. } => "Input should be a valid date in the format YYYY-MM-DD, {error}",
            Self::DateFromDatetimeParsing { .. } => "Input should be a valid date or datetime, {error}",
            Self::DateFromDatetimeInexact { .. } => {
                "Datetimes provided to dates should have zero time - e.g. be exact dates"
            }
            Self::DatePast { .. } => "Date should be in the past",
            Self::DateFuture { .. } => "Date should be in the future",
            Self::TimeType { .. } => "Input should be a valid time",
            Self::TimeParsing { .. } => "Input should be in a valid time format, {error}",
            Self::DatetimeType { .. } => "Input should be a valid datetime",
            Self::DatetimeParsing { .. } => "Input should be a valid datetime, {error}",
            Self::DatetimeObjectInvalid { .. } => "Invalid datetime object, got {error}",
            Self::DatetimeFromDateParsing { .. } => "Input should be a valid datetime or date, {error}",
            Self::DatetimePast { .. } => "Input should be in the past",
            Self::DatetimeFuture { .. } => "Input should be in the future",
            Self::TimezoneNaive { .. } => "Input should not have timezone info",
            Self::TimezoneAware { .. } => "Input should have timezone info",
            Self::TimezoneOffset { .. } => "Timezone offset of {tz_expected} required, got {tz_actual}",
            Self::TimeDeltaType { .. } => "Input should be a valid timedelta",
            Self::TimeDeltaParsing { .. } => "Input should be a valid timedelta, {error}",
            Self::FrozenSetType { .. } => "Input should be a valid frozenset",
            Self::IsInstanceOf { .. } => "Input should be an instance of {class}",
            Self::IsSubclassOf { .. } => "Input should be a subclass of {class}",
            Self::CallableType { .. } => "Input should be callable",
            Self::UnionTagInvalid { .. } => {
                "Input tag '{tag}' found using {discriminator} does not match any of the expected tags: {expected_tags}"
            }
            Self::UnionTagNotFound { .. } => "Unable to extract tag using discriminator {discriminator}",
            Self::ArgumentsType { .. } => "Arguments must be a tuple, list or a dictionary",
            Self::MissingArgument { .. } => "Missing required argument",
            Self::UnexpectedKeywordArgument { .. } => "Unexpected keyword argument",
            Self::MissingKeywordOnlyArgument { .. } => "Missing required keyword only argument",
            Self::UnexpectedPositionalArgument { .. } => "Unexpected positional argument",
            Self::MissingPositionalOnlyArgument { .. } => "Missing required positional only argument",
            Self::MultipleArgumentValues { .. } => "Got multiple values for argument",
            Self::UrlType { .. } => "URL input should be a string or URL",
            Self::UrlParsing { .. } => "Input should be a valid URL, {error}",
            Self::UrlSyntaxViolation { .. } => "Input violated strict URL syntax rules, {error}",
            Self::UrlTooLong { .. } => "URL should have at most {max_length} character{expected_plural}",
            Self::UrlScheme { .. } => "URL scheme should be {expected_schemes}",
            Self::UuidType { .. } => "UUID input should be a string, bytes or UUID object",
            Self::UuidParsing { .. } => "Input should be a valid UUID, {error}",
            Self::UuidVersion { .. } => "UUID version {expected_version} expected",
            Self::DecimalType { .. } => "Decimal input should be an integer, float, string or Decimal object",
            Self::DecimalParsing { .. } => "Input should be a valid decimal",
            Self::DecimalMaxDigits { .. } => {
                "Decimal input should have no more than {max_digits} digit{expected_plural} in total"
            }
            Self::DecimalMaxPlaces { .. } => {
                "Decimal input should have no more than {decimal_places} decimal place{expected_plural}"
            }
            Self::DecimalWholeDigits { .. } => {
                "Decimal input should have no more than {whole_digits} digit{expected_plural} before the decimal point"
            }
            Self::FractionType { .. } => "Fraction input should be an integer, float, string or Fraction object",
            Self::FractionParsing { .. } => "Input is not a valid fraction",
            Self::ComplexType { .. } => {
                "Input should be a valid python complex object, a number, or a valid complex string following the rules at https://docs.python.org/3/library/functions.html#complex"
            }
            Self::ComplexStrParsing { .. } => {
                "Input should be a valid complex string following the rules at https://docs.python.org/3/library/functions.html#complex"
            }
        }
    }

    /// The message with the JSON wording, which differs for the types whose
    /// Python wording names a Python type.
    fn message_template_json(&self) -> &'static str {
        match self {
            Self::NoneRequired { .. } => "Input should be null",
            Self::ListType { .. }
            | Self::DequeType { .. }
            | Self::TupleType { .. }
            | Self::IterableType { .. }
            | Self::SetType { .. }
            | Self::FrozenSetType { .. } => "Input should be a valid array",
            Self::ModelType { .. }
            | Self::ModelAttributesType { .. }
            | Self::DictType { .. }
            | Self::FrozenDictType { .. }
            | Self::OrderedDictType { .. }
            | Self::CounterType { .. }
            | Self::DataclassType { .. } => "Input should be an object",
            Self::NamedTupleType { .. } => "Input should be an array or an object",
            Self::TimeDeltaType { .. } => "Input should be a valid duration",
            Self::TimeDeltaParsing { .. } => "Input should be a valid duration, {error}",
            Self::ArgumentsType { .. } => "Arguments must be an array or an object",
            _ => self.message_template_python(),
        }
    }

    /// The rendered message for `input_type`: the template with each
    /// `{field}` replaced, plus the `{expected_plural}` suffix.
    pub(crate) fn render_message(&self, input_type: ValidationInputType) -> String {
        let template = match input_type {
            ValidationInputType::Python => self.message_template_python(),
            ValidationInputType::Json => self.message_template_json(),
        };
        let mut message = template.to_owned();
        for (field, value) in self.context() {
            let placeholder = format!("{{{field}}}");
            if message.contains(&placeholder) {
                message = message.replace(&placeholder, &context_display(&value));
            }
        }
        if let Some(count) = self.plural_count() {
            message = message.replace("{expected_plural}", if count == 1 { "" } else { "s" });
        }
        message
    }

    /// The count a template's `{expected_plural}` agrees with.
    fn plural_count(&self) -> Option<u64> {
        match self {
            Self::TooShort { min_length, .. }
            | Self::StringTooShort { min_length, .. }
            | Self::BytesTooShort { min_length, .. } => Some(*min_length as u64),
            Self::TooLong { max_length, .. }
            | Self::StringTooLong { max_length, .. }
            | Self::BytesTooLong { max_length, .. }
            | Self::UrlTooLong { max_length, .. } => Some(*max_length as u64),
            Self::DecimalMaxDigits { max_digits, .. } => Some(*max_digits),
            Self::DecimalMaxPlaces { decimal_places, .. } => Some(*decimal_places),
            Self::DecimalWholeDigits { whole_digits, .. } => Some(*whole_digits),
            _ => None,
        }
    }
}

/// How a context value appears inside a message: strings verbatim, numbers
/// as Python prints them, an absent `too_long` length as `more`.
fn context_display(value: &MontyObject) -> String {
    let value = value.as_ref();
    if let Some(s) = value.as_str() {
        s.to_owned()
    } else if value.type_name() == "NoneType" {
        "more".to_owned()
    } else {
        value.py_repr()
    }
}

/// A numeric constraint such as `gt`, kept as the number the schema gave.
pub(crate) type Number = MontyObject;

/// A context field's conversions: from a `from_exception_data` context dict,
/// and to the `ctx` entry `errors()` reports.
pub(crate) trait CtxField: Sized {
    /// Reads and converts the entry named `field`, with pydantic's errors for a
    /// missing or mistyped one.
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self>;

    /// The value as `errors()` reports it.
    fn to_context(&self) -> MontyObject;
}

/// Reads a required context entry, converting it with `convert`; a missing
/// entry and a failed conversion raise the `TypeError`s pydantic raises.
fn required_context<T>(
    ctx: &[(String, Value)],
    field: &str,
    variant: &str,
    type_name: &str,
    vm: &mut VM<'_>,
    convert: impl FnOnce(&Value, &mut VM<'_>) -> Option<T>,
) -> RunResult<T> {
    let Some((_, value)) = ctx.iter().find(|(key, _)| key == field) else {
        return Err(ExcType::type_error(format!("{variant}: '{field}' required in context")));
    };
    convert(value, vm)
        .ok_or_else(|| ExcType::type_error(format!("{variant}: '{field}' context value must be a {type_name}")))
}

impl CtxField for String {
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self> {
        required_context(ctx, field, variant, "String", vm, |value, vm| {
            value.as_str(vm).map(str::to_owned)
        })
    }

    fn to_context(&self) -> MontyObject {
        MontyObject::string(self.clone())
    }
}

impl CtxField for usize {
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self> {
        required_context(ctx, field, variant, "usize", vm, |value, _| match value {
            Value::Int(n) => Self::try_from(*n).ok(),
            _ => None,
        })
    }

    fn to_context(&self) -> MontyObject {
        MontyObject::int(i64::try_from(*self).unwrap_or(i64::MAX))
    }
}

impl CtxField for u64 {
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self> {
        required_context(ctx, field, variant, "u64", vm, |value, _| match value {
            Value::Int(n) => Self::try_from(*n).ok(),
            _ => None,
        })
    }

    fn to_context(&self) -> MontyObject {
        MontyObject::int(i64::try_from(*self).unwrap_or(i64::MAX))
    }
}

impl CtxField for i64 {
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self> {
        required_context(ctx, field, variant, "i32", vm, |value, _| match value {
            Value::Int(n) => Some(*n),
            _ => None,
        })
    }

    fn to_context(&self) -> MontyObject {
        MontyObject::int(*self)
    }
}

impl CtxField for Option<usize> {
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self> {
        required_context(ctx, field, variant, "Option<usize>", vm, |value, _| match value {
            Value::None => Some(None),
            Value::Int(n) => usize::try_from(*n).ok().map(Some),
            _ => None,
        })
    }

    fn to_context(&self) -> MontyObject {
        self.map_or_else(MontyObject::none, |n| {
            MontyObject::int(i64::try_from(n).unwrap_or(i64::MAX))
        })
    }
}

impl CtxField for MontyObject {
    fn from_context(ctx: &[(String, Value)], field: &str, variant: &str, vm: &mut VM<'_>) -> RunResult<Self> {
        required_context(ctx, field, variant, "Number", vm, |value, vm| match value {
            Value::Int(_) | Value::Float(_) | Value::InternLongInt(_) => Some(snapshot_value(value, vm)),
            Value::Ref(_) if value.as_str(vm).is_none() => Some(snapshot_value(value, vm)),
            _ => None,
        })
    }

    fn to_context(&self) -> MontyObject {
        self.clone()
    }
}
