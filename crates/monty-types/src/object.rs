//! The leaf payloads of a boundary value: [`MontyType`], the datetime value
//! types, [`MontyFileHandle`], and the errors reading or importing one raises.

use std::{
    borrow::Cow,
    error::Error,
    fmt,
    hash::{Hash, Hasher},
};

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta as ChronoTimeDelta};

use crate::{exceptions::ExcType, file_mode::FileMode, format::StringRepr, resource::ResourceError};

/// The Python type of a builtin at the host boundary — the public mirror of
/// the internal runtime `Type` enum, minus class types, which cross as their
/// own [`ClassType`](crate::MontyNode::ClassType) arena node.
///
/// Self-contained: it can be serialized, sent over the subprocess wire
/// protocol, and displayed without heap access.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumIter,
    strum::EnumString,
    strum::IntoStaticStr,
    strum::VariantNames,
)]
#[strum(serialize_all = "lowercase")]
pub enum MontyType {
    Ellipsis,
    Type,
    #[strum(serialize = "NoneType")]
    NoneType,
    Bool,
    Int,
    Float,
    Range,
    Slice,
    /// The four `datetime` classes carry the qualified names the runtime
    /// `Type` uses (`datetime.date`, ...) rather than bare `date`, so a type
    /// object keeps one name either side of the boundary.
    #[strum(serialize = "datetime.date")]
    Date,
    #[strum(serialize = "datetime.datetime")]
    DateTime,
    #[strum(serialize = "datetime.timedelta")]
    TimeDelta,
    #[strum(serialize = "datetime.timezone")]
    TimeZone,
    Str,
    Bytes,
    List,
    /// `collections.deque`. Qualified like `datetime.datetime` so the
    /// host-boundary name matches the runtime `Type::Deque` (`collections.deque`)
    /// rather than a bare `deque`.
    #[strum(serialize = "collections.deque")]
    Deque,
    #[strum(serialize = "list_iterator")]
    ListIterator,
    #[strum(serialize = "callable_iterator")]
    CallableIterator,
    Tuple,
    NamedTuple,
    Dict,
    #[strum(serialize = "dict_keys")]
    DictKeys,
    #[strum(serialize = "dict_items")]
    DictItems,
    #[strum(serialize = "dict_values")]
    DictValues,
    Set,
    FrozenSet,
    /// Exception types render/parse via `ExcType`'s own strum name
    /// (`"ValueError"`, `"json.JSONDecodeError"`, ...), so this variant is
    /// `#[strum(disabled)]`: [`name`](Self::name) and
    /// [`from_type_name`](Self::from_type_name) peel `Exception` off
    /// explicitly.
    #[strum(disabled)]
    Exception(ExcType),
    Function,
    #[strum(serialize = "builtin_function_or_method")]
    BuiltinFunction,
    Cell,
    Iterator,
    Coroutine,
    Module,
    #[strum(serialize = "_io.TextIOWrapper")]
    TextIOWrapper,
    #[strum(serialize = "_io.BufferedReader")]
    BufferedReader,
    #[strum(serialize = "_io.BufferedWriter")]
    BufferedWriter,
    #[strum(serialize = "_io.BufferedRandom")]
    BufferedRandom,
    #[strum(serialize = "typing._SpecialForm")]
    SpecialForm,
    #[strum(serialize = "PosixPath")]
    Path,
    Property,
    #[strum(serialize = "re.Pattern")]
    RePattern,
    #[strum(serialize = "re.Match")]
    ReMatch,
    // Serialized enum variants are append-only to preserve postcard discriminants.
    #[strum(serialize = "tuple_iterator")]
    TupleIterator,
    #[strum(serialize = "str_ascii_iterator")]
    StrAsciiIterator,
    #[strum(serialize = "str_iterator")]
    StrIterator,
    #[strum(serialize = "bytes_iterator")]
    BytesIterator,
    #[strum(serialize = "range_iterator")]
    RangeIterator,
    #[strum(serialize = "dict_keyiterator")]
    DictKeyIterator,
    #[strum(serialize = "dict_itemiterator")]
    DictItemIterator,
    #[strum(serialize = "dict_valueiterator")]
    DictValueIterator,
    #[strum(serialize = "set_iterator")]
    SetIterator,
    #[strum(serialize = "itertools.count")]
    ItertoolsCount,
    #[strum(serialize = "itertools.repeat")]
    ItertoolsRepeat,
    /// A `dataclasses.Field` describing one field of a sandbox `@dataclass`,
    /// as found in a class's `__dataclass_fields__`.
    #[strum(serialize = "Field")]
    Field,
    #[strum(serialize = "itertools.pairwise")]
    ItertoolsPairwise,
    #[strum(serialize = "itertools.compress")]
    ItertoolsCompress,
    #[strum(serialize = "itertools.islice")]
    ItertoolsIslice,
    #[strum(serialize = "itertools.chain")]
    ItertoolsChain,
    #[strum(serialize = "itertools.cycle")]
    ItertoolsCycle,
    #[strum(serialize = "NotImplementedType")]
    NotImplementedType,
    /// The `__dataclass_params__` of a sandbox `@dataclass`: the options it was
    /// decorated with, named as CPython's private class reports itself.
    #[strum(serialize = "_DataclassParams")]
    DataclassParams,
    #[strum(serialize = "itertools.takewhile")]
    ItertoolsTakeWhile,
    #[strum(serialize = "itertools.dropwhile")]
    ItertoolsDropWhile,
    #[strum(serialize = "itertools.filterfalse")]
    ItertoolsFilterFalse,
    #[strum(serialize = "itertools.starmap")]
    ItertoolsStarMap,
    /// The builtin `object`, which the sandbox exposes as a name only — it is
    /// not a base class and cannot be constructed.
    Object,
    #[strum(serialize = "datetime.time")]
    Time,
    /// `functools.partial`, qualified the way CPython's `tp_name` is.
    #[strum(serialize = "functools.partial")]
    Partial,
    #[strum(serialize = "itertools.accumulate")]
    ItertoolsAccumulate,
    #[strum(serialize = "itertools.batched")]
    ItertoolsBatched,
    #[strum(serialize = "itertools.zip_longest")]
    ItertoolsZipLongest,
    /// `types.GenericAlias`, the type of `list[int]`, qualified the way CPython's `tp_name` is.
    #[strum(serialize = "types.GenericAlias")]
    GenericAlias,
    /// `typing.Union`, the type of `int | None` (one object with `types.UnionType` since 3.14).
    #[strum(serialize = "typing.Union")]
    Union,
    #[strum(serialize = "itertools.combinations")]
    ItertoolsCombinations,
    #[strum(serialize = "itertools.combinations_with_replacement")]
    ItertoolsCombinationsWithReplacement,
    #[strum(serialize = "itertools.permutations")]
    ItertoolsPermutations,
    #[strum(serialize = "itertools.product")]
    ItertoolsProduct,
    #[strum(serialize = "itertools.groupby")]
    ItertoolsGroupBy,
    #[strum(serialize = "itertools._grouper")]
    ItertoolsGrouper,
    #[strum(serialize = "itertools._tee")]
    ItertoolsTee,
    #[strum(serialize = "itertools._tee_dataobject")]
    ItertoolsTeeDataObject,
}

impl fmt::Display for MontyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl MontyType {
    /// The Python-visible name of this type (`"int"`, `"datetime.datetime"`,
    /// `"ValueError"`).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Exception(exc_type) => (*exc_type).into(),
            // Every remaining variant is named by strum's `IntoStaticStr`
            // (`Exception` is peeled off above).
            other => other.into(),
        }
    }

    /// Parses a name produced by [`Display`](fmt::Display)/[`name`](Self::name)
    /// back to the [`MontyType`] — the wire-protocol decode path for builtin
    /// type names. A non-builtin class name returns `None` (a class crosses
    /// as its own arena node instead), and `"object"` parses to the builtin
    /// [`Object`](Self::Object).
    ///
    /// `EnumString` parses via the same strum `serialize` attributes that
    /// `IntoStaticStr` renders with, so the two stay in lockstep by
    /// construction. Exception types display as their exception name
    /// ("ValueError", "json.JSONDecodeError", ...) — fall back to the
    /// [`ExcType`](crate::ExcType) parser.
    #[must_use]
    pub fn from_type_name(name: &str) -> Option<Self> {
        name.parse::<Self>()
            .ok()
            .or_else(|| name.parse::<ExcType>().ok().map(Self::Exception))
    }
}

/// A Python `datetime.date` value with year, month, and day components.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MontyDate {
    /// Gregorian year in range 1..=9999.
    pub year: i32,
    /// Month component in range 1..=12.
    pub month: u8,
    /// Day component valid for the given month/year.
    pub day: u8,
}

/// A Python `datetime.datetime` value with date, time, and optional timezone components.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyDateTime {
    /// Gregorian year in range 1..=9999.
    pub year: i32,
    /// Month component in range 1..=12.
    pub month: u8,
    /// Day component valid for the given month/year.
    pub day: u8,
    /// Hour in range 0..=23.
    pub hour: u8,
    /// Minute in range 0..=59.
    pub minute: u8,
    /// Second in range 0..=59.
    pub second: u8,
    /// Microsecond in range 0..=999_999.
    pub microsecond: u32,
    /// Fixed offset seconds for aware datetimes, or `None` for naive values.
    ///
    /// Within [`MIN_TIMEZONE_OFFSET_SECONDS`]..=[`MAX_TIMEZONE_OFFSET_SECONDS`] when set.
    pub offset_seconds: Option<i32>,
    /// Optional explicit timezone name for aware datetimes.
    ///
    /// Must be `None` when `offset_seconds` is `None`.
    pub timezone_name: Option<String>,
}

/// A Python `datetime.time` value: a wall clock with no date attached.
///
/// `fold` is carried so the flag survives the boundary, but neither monty nor
/// this type interprets it — as in CPython it takes no part in equality.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyTime {
    /// Hour in range 0..=23.
    pub hour: u8,
    /// Minute in range 0..=59.
    pub minute: u8,
    /// Second in range 0..=59.
    pub second: u8,
    /// Microsecond in range 0..=999_999.
    pub microsecond: u32,
    /// Fixed offset seconds for aware times, or `None` for naive values.
    ///
    /// Within [`MIN_TIMEZONE_OFFSET_SECONDS`]..=[`MAX_TIMEZONE_OFFSET_SECONDS`] when set.
    pub offset_seconds: Option<i32>,
    /// Optional explicit timezone name for aware times.
    ///
    /// Must be `None` when `offset_seconds` is `None`.
    pub timezone_name: Option<String>,
    /// Fold flag, 0 or 1.
    pub fold: u8,
}

/// A Python `datetime.timedelta` value representing a duration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MontyTimeDelta {
    /// Day component.
    pub days: i32,
    /// Seconds component in normalized range 0..86400.
    pub seconds: i32,
    /// Microseconds component in normalized range 0..1_000_000.
    pub microseconds: i32,
}

/// Smallest UTC offset `datetime.timezone` accepts, -23:59:59.
///
/// CPython requires an offset strictly inside ±24 hours. Shared with the wire
/// decoder so a forged offset is rejected at the boundary rather than by the
/// sandbox-side constructor, which by then can only report a generic bad value.
pub const MIN_TIMEZONE_OFFSET_SECONDS: i32 = -86_399;
/// Largest UTC offset `datetime.timezone` accepts, +23:59:59.
///
/// See [`MIN_TIMEZONE_OFFSET_SECONDS`].
pub const MAX_TIMEZONE_OFFSET_SECONDS: i32 = 86_399;

/// A Python `datetime.timezone` fixed-offset timezone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyTimeZone {
    /// Fixed UTC offset in seconds, within [`MIN_TIMEZONE_OFFSET_SECONDS`]..=[`MAX_TIMEZONE_OFFSET_SECONDS`].
    pub offset_seconds: i32,
    /// Optional display name.
    pub name: Option<String>,
}

/// Wall-clock microseconds since midnight, before any offset is applied.
///
/// Every field is range-bounded by its type, so this is total and cannot
/// overflow — unlike the datetime equivalent, which can fail on an invalid date.
fn monty_time_local_micros(time: &MontyTime) -> i64 {
    i64::from(time.hour) * 3_600_000_000
        + i64::from(time.minute) * 60_000_000
        + i64::from(time.second) * 1_000_000
        + i64::from(time.microsecond)
}

/// Comparison key: offset-adjusted microseconds for an aware time, wall-clock
/// microseconds for a naive one.
///
/// The adjusted value is deliberately NOT wrapped into a 24-hour day — a bare
/// time has no date to carry into, so `time(1, 0, utc)` differs from
/// `time(23, 0, minus_two)`, as in CPython.
fn monty_time_key(time: &MontyTime) -> i64 {
    monty_time_local_micros(time) - i64::from(time.offset_seconds.unwrap_or(0)) * 1_000_000
}

/// Aware and naive times never compare equal, and `fold` takes no part —
/// both matching CPython.
impl PartialEq for MontyTime {
    fn eq(&self, other: &Self) -> bool {
        self.offset_seconds.is_some() == other.offset_seconds.is_some() && monty_time_key(self) == monty_time_key(other)
    }
}

impl Eq for MontyTime {}

impl Hash for MontyTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Must agree with `PartialEq`: awareness and the adjusted key only,
        // never `fold`.
        self.offset_seconds.is_some().hash(state);
        monty_time_key(self).hash(state);
    }
}

impl PartialEq for MontyDateTime {
    fn eq(&self, other: &Self) -> bool {
        let self_aware = self.offset_seconds.is_some();
        let other_aware = other.offset_seconds.is_some();
        if self_aware != other_aware {
            return false;
        }

        if self_aware {
            return monty_datetime_utc_micros(self)
                .zip(monty_datetime_utc_micros(other))
                .is_some_and(|(lhs, rhs)| lhs == rhs)
                || monty_datetime_raw_eq(self, other);
        }

        monty_datetime_local_micros(self)
            .zip(monty_datetime_local_micros(other))
            .is_some_and(|(lhs, rhs)| lhs == rhs)
            || monty_datetime_raw_eq(self, other)
    }
}

impl Eq for MontyDateTime {}

impl Hash for MontyDateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.offset_seconds.is_some()
            && let Some(utc_micros) = monty_datetime_utc_micros(self)
        {
            utc_micros.hash(state);
            return;
        }
        if let Some(local_micros) = monty_datetime_local_micros(self) {
            local_micros.hash(state);
            return;
        }

        // Invalid carrier values should still hash deterministically instead of panicking.
        self.year.hash(state);
        self.month.hash(state);
        self.day.hash(state);
        self.hour.hash(state);
        self.minute.hash(state);
        self.second.hash(state);
        self.microsecond.hash(state);
        self.offset_seconds.hash(state);
        self.timezone_name.hash(state);
    }
}

impl PartialEq for MontyTimeZone {
    fn eq(&self, other: &Self) -> bool {
        self.offset_seconds == other.offset_seconds
    }
}

impl Eq for MontyTimeZone {}

impl Hash for MontyTimeZone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.offset_seconds.hash(state);
    }
}

/// Error returned when a [`MontyObject`] cannot be converted to the requested Rust type.
///
/// This error is returned by the `TryFrom` implementations when attempting to extract
/// a specific type from a [`ValueRef`](crate::ValueRef) that holds a different kind of value.
#[derive(Debug)]
pub struct ConversionError {
    /// The type name that was expected (e.g., "int", "str").
    pub expected: &'static str,
    /// The actual type name of the value (e.g., "list", "NoneType", or a
    /// class instance's class name).
    pub actual: String,
}

impl ConversionError {
    /// Creates a new [`ConversionError`] with the expected and actual type names.
    #[must_use]
    pub fn new(expected: &'static str, actual: impl Into<String>) -> Self {
        Self {
            expected,
            actual: actual.into(),
        }
    }
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expected {}, got {}", self.expected, self.actual)
    }
}

impl Error for ConversionError {}

/// Error returned when a value cannot be used as an input to code execution.
///
/// This can occur when:
/// - A node kind (like [`Repr`](crate::MontyNode::Repr)) is only valid as an output, not an input
/// - A resource limit is exceeded during conversion
#[derive(Debug, Clone)]
pub enum InvalidInputError {
    /// The input type is not valid for conversion to a runtime Value.
    /// Message explaining why the type is invalid.
    InvalidType(Cow<'static, str>),
    /// A resource limit was exceeded during conversion.
    Resource(ResourceError),
}

impl InvalidInputError {
    /// Creates a new [`InvalidInputError`] for the given type name.
    #[must_use]
    pub fn invalid_type(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::InvalidType(msg.into())
    }
}

impl fmt::Display for InvalidInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType(msg) => write!(f, "{msg}"),
            Self::Resource(e) => write!(f, "{e}"),
        }
    }
}

impl Error for InvalidInputError {}

impl From<ResourceError> for InvalidInputError {
    fn from(err: ResourceError) -> Self {
        Self::Resource(err)
    }
}

/// An open file object (the result of `open()`).
///
/// This is the boundary representation of Monty's heap `OpenFile`
/// wrapper. It carries everything needed to service a file operation from a
/// host that holds no live OS handle: the virtual `path`, the `mode`, and
/// the byte `position` for seek-aware reads.
///
/// The host produces a `FileHandle` as the result of an
/// [`OsFunctionCall::Open`](crate::os::OsFunctionCall::Open) call; the
/// interpreter then builds its heap file wrapper from it. Conversely, a heap file
/// object passed as an argument to a `read`/`write` OS call is converted
/// back to a `FileHandle` so the host receives this state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MontyFileHandle {
    /// The virtual (sandbox) path of the file. Never a host path.
    pub path: String,
    /// The parsed `open()` mode.
    pub mode: FileMode,
    /// Position for sized/line/seek operations: char index in text mode,
    /// byte index in binary mode. `0` for a freshly opened file.
    pub position: u64,
}

impl fmt::Display for MontyFileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<{} name={} mode={}>",
            self.mode.file_type_name(),
            StringRepr(&self.path),
            StringRepr(self.mode.as_str())
        )
    }
}

fn monty_datetime_local_micros(datetime: &MontyDateTime) -> Option<i64> {
    monty_datetime_naive(datetime).map(|naive| naive.and_utc().timestamp_micros())
}

fn monty_datetime_raw_eq(a: &MontyDateTime, b: &MontyDateTime) -> bool {
    a.year == b.year
        && a.month == b.month
        && a.day == b.day
        && a.hour == b.hour
        && a.minute == b.minute
        && a.second == b.second
        && a.microsecond == b.microsecond
        && a.offset_seconds == b.offset_seconds
        && a.timezone_name == b.timezone_name
}

fn monty_datetime_utc_micros(datetime: &MontyDateTime) -> Option<i64> {
    let offset_seconds = datetime.offset_seconds?;
    let offset_delta = ChronoTimeDelta::try_seconds(i64::from(offset_seconds))?;
    let utc = monty_datetime_naive(datetime)?.checked_sub_signed(offset_delta)?;
    Some(utc.and_utc().timestamp_micros())
}

fn monty_datetime_naive(datetime: &MontyDateTime) -> Option<NaiveDateTime> {
    let date = NaiveDate::from_ymd_opt(datetime.year, u32::from(datetime.month), u32::from(datetime.day))?;
    let time = NaiveTime::from_hms_micro_opt(
        u32::from(datetime.hour),
        u32::from(datetime.minute),
        u32::from(datetime.second),
        datetime.microsecond,
    )?;
    Some(date.and_time(time))
}
