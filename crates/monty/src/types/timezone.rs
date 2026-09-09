//! Python `datetime.timezone` implementation for fixed-offset zones.
//!
//! Phase 1 intentionally supports only fixed offsets (no DST or IANA database).

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

// The bounds live in `monty-types` so the wire decoder shares them; re-exported
// here because this is where the sandbox-side constructor enforces them.
pub(crate) use monty_types::{MAX_TIMEZONE_OFFSET_SECONDS, MIN_TIMEZONE_OFFSET_SECONDS};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    types::{
        LazyHeapSet, PyTrait, Type,
        str::{StringRepr, allocate_string},
        timedelta,
        timedelta::{MICROSECONDS_PER_SECOND, SECONDS_PER_HOUR, SECONDS_PER_MINUTE},
    },
    value::{EitherStr, Value},
};

/// Magnitude of the `timezone.min` / `timezone.max` class constants: 23:59.
///
/// Whole minutes, unlike [`MAX_TIMEZONE_OFFSET_SECONDS`]: CPython defines the
/// constants as `±timedelta(hours=23, minutes=59)` while still *accepting*
/// sub-minute offsets, so the two bounds differ by 59 seconds.
pub(crate) const MAX_TIMEZONE_CONSTANT_SECONDS: i32 = 86_340;

/// Python `datetime.timezone` value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimeZone {
    /// Fixed offset in seconds from UTC.
    pub offset_seconds: i32,
    /// Optional display name.
    pub name: Option<String>,
}

impl TimeZone {
    /// Creates a new fixed-offset timezone after validating CPython-compatible bounds.
    pub fn new(offset_seconds: i32, name: Option<String>) -> RunResult<Self> {
        if !(MIN_TIMEZONE_OFFSET_SECONDS..=MAX_TIMEZONE_OFFSET_SECONDS).contains(&offset_seconds) {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not datetime.timedelta(seconds={offset_seconds})"
                ),
            )
            .into());
        }
        Ok(Self { offset_seconds, name })
    }

    /// Returns the canonical UTC timezone singleton value.
    #[must_use]
    pub fn utc() -> Self {
        Self {
            offset_seconds: 0,
            name: None,
        }
    }

    /// Parses timezone constructor arguments.
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let TimezoneInitArgs { offset, name } = TimezoneInitArgs::from_args(args, vm)?;
        // Keep `offset` and `name` alive across the validation helpers — they
        // own the heap refs (TimeDelta / Str) we're reading from. `name` is
        // an `Option<Value>` so we can distinguish "omitted" from an explicit
        // `None`: CPython accepts `timezone(td)` but rejects `timezone(td,
        // None)` with `TypeError: timezone() argument 2 must be str, not None`.
        defer_drop!(offset, vm);
        let offset_seconds = extract_offset_seconds(offset, vm.heap, vm.interns)?;
        let name_str: Option<String> = match name {
            None => None,
            Some(name) => {
                defer_drop!(name, vm);
                extract_name(name, vm.heap, vm.interns)?
            }
        };

        if offset_seconds == 0 && name_str.is_none() {
            return Ok(vm.heap.get_timezone_utc());
        }

        let tz = Self::new(offset_seconds, name_str)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::TimeZone(tz))))
    }
}

/// Argument shape for `timezone(offset, name=None)`.
///
/// `timezone` is a C-implemented constructor that emits its function name in
/// error messages (unlike `datetime`, which uses the bare `"function"`
/// label). Hence `style = c_named`.
///
/// Both `offset` and `name` are held as `Value` so the inner code can do its
/// own custom validation (`offset` must be a `timedelta`; `name` must be a
/// `str`). The macro only handles arg-count/keyword dispatch.
#[derive(FromArgs)]
#[from_args(name = "timezone", style = c_named, at_most_total)]
struct TimezoneInitArgs {
    offset: Value,
    // `Option<Value>` (with `default`) preserves the distinction between
    // omitted (`None`) and explicitly passed `None` (`Some(Value::None)`),
    // which `extract_name` needs to reject the latter.
    #[from_args(default)]
    name: Option<Value>,
}

impl PartialEq for TimeZone {
    fn eq(&self, other: &Self) -> bool {
        self.offset_seconds == other.offset_seconds
    }
}

impl Eq for TimeZone {}

impl Hash for TimeZone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // CPython timezone equality/hash are offset-based.
        self.offset_seconds.hash(state);
    }
}

fn extract_offset_seconds(offset_arg: &Value, heap: &Heap, interns: &Interns) -> RunResult<i32> {
    let bad_type = || {
        ExcType::type_error(format!(
            "timezone() argument 1 must be datetime.timedelta, not {}",
            offset_arg.py_type_heap(heap).cpython_arg_name(heap, interns),
        ))
    };
    let Value::Ref(offset_id) = offset_arg else {
        return Err(bad_type());
    };
    let HeapData::TimeDelta(delta) = heap.get(*offset_id) else {
        return Err(bad_type());
    };

    let Some(total_seconds) = timedelta::exact_total_seconds(delta) else {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "offset must be a timedelta representing a whole number of seconds",
        )
        .into());
    };

    if !(i128::from(MIN_TIMEZONE_OFFSET_SECONDS)..=i128::from(MAX_TIMEZONE_OFFSET_SECONDS)).contains(&total_seconds) {
        let timedelta_repr = timedelta::format_repr(delta);
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!(
                "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not {timedelta_repr}"
            ),
        )
        .into());
    }

    i32::try_from(total_seconds)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "timezone offset out of range").into())
}

/// Formats a generic offset as `+HH:MM` or `+HH:MM:SS`.
#[must_use]
pub(crate) fn format_offset_hms(offset_seconds: i32) -> String {
    let sign = if offset_seconds >= 0 { '+' } else { '-' };
    let abs = offset_seconds.abs();
    let hours = abs / SECONDS_PER_HOUR;
    let minutes = (abs % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let seconds = abs % SECONDS_PER_MINUTE;
    if seconds == 0 {
        return format!("{sign}{hours:02}:{minutes:02}");
    }
    format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
}

/// The name a fixed-offset zone reports from `tzname()` and `str()`.
///
/// An explicit constructor name wins; otherwise CPython renders the zero offset
/// as the bare `UTC` and every other offset as `UTC±HH:MM[:SS]`.
#[must_use]
pub(crate) fn tzname_string(offset_seconds: i32, name: Option<&str>) -> String {
    match name {
        Some(name) => name.to_owned(),
        None if offset_seconds == 0 => "UTC".to_owned(),
        None => format!("UTC{}", format_offset_hms(offset_seconds)),
    }
}

/// Builds the value `utcoffset()` returns: a `timedelta` for a fixed offset,
/// `None` for a naive value. Shared by `timezone`, `datetime` and `time`.
pub(crate) fn utcoffset_value(offset_seconds: Option<i32>, heap: &Heap) -> Value {
    match offset_seconds {
        None => Value::None,
        Some(offset_seconds) => timedelta::allocate_micros(i128::from(offset_seconds) * MICROSECONDS_PER_SECOND, heap),
    }
}

/// Allocates an unnamed `timezone` for an offset known to be in range.
///
/// For the `timezone.min` / `timezone.max` class constants; anything derived
/// from user input must go through [`TimeZone::new`] so the bounds are checked.
pub(crate) fn allocate_offset(offset_seconds: i32, heap: &Heap) -> Value {
    let tz = TimeZone::new(offset_seconds, None).expect("caller guarantees an in-range offset");
    Value::Ref(heap.allocate(HeapData::TimeZone(tz)))
}

/// Validates the `dt` argument every `tzinfo` method takes.
///
/// The offset is fixed, so the argument is never read. CPython still requires
/// it to be a `datetime` or `None`, and so does this.
fn check_tzinfo_dt_arg(method: &str, dt: &Value, heap: &Heap, interns: &Interns) -> RunResult<()> {
    match dt {
        Value::None => Ok(()),
        Value::Ref(id) if matches!(heap.get(*id), HeapData::DateTime(_)) => Ok(()),
        _ => Err(ExcType::type_error_tzinfo_dt_arg(
            method,
            &dt.py_type_heap(heap).cpython_arg_name(heap, interns),
        )),
    }
}

/// Formats a canonical `datetime.timedelta(...)` repr for a fixed offset in seconds.
#[must_use]
pub(crate) fn format_offset_timedelta_repr(offset_seconds: i32) -> String {
    let delta = timedelta::from_total_microseconds(i128::from(offset_seconds) * MICROSECONDS_PER_SECOND)
        .expect("timezone offset range is always representable as timedelta");
    timedelta::format_repr(&delta)
}

fn extract_name(name_arg: &Value, heap: &Heap, interns: &Interns) -> RunResult<Option<String>> {
    match name_arg {
        Value::InternString(id) => Ok(Some(interns.get_str(*id).to_owned())),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => Ok(Some(s.as_str().to_owned())),
            _ => Err(bad_name_arg(name_arg, heap, interns)),
        },
        _ => Err(bad_name_arg(name_arg, heap, interns)),
    }
}

/// Builds the `timezone() argument 2 must be str, not <type>` error CPython
/// raises for any non-`str` `name` argument (including explicit `None`).
fn bad_name_arg(name_arg: &Value, heap: &Heap, interns: &Interns) -> RunError {
    ExcType::type_error(format!(
        "timezone() argument 2 must be str, not {}",
        name_arg.py_type_heap(heap).cpython_arg_name(heap, interns)
    ))
}

impl HeapItem for TimeZone {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// `HeapRead`-based dispatch for `TimeZone`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident timezone objects.
impl<'h> PyTrait<'h> for HeapObjectRead<'h, TimeZone> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::TimeZone
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::TimeZone(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(
            self.get(vm.heap).offset_seconds == other.get(vm.heap).offset_seconds,
        ))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let tz = self.get(vm.heap);
        if tz.offset_seconds == 0 && tz.name.is_none() {
            f.write_str("datetime.timezone.utc")?;
            return Ok(());
        }

        let timedelta_repr = format_offset_timedelta_repr(tz.offset_seconds);
        write!(f, "datetime.timezone({timedelta_repr}")?;
        if let Some(name) = &tz.name {
            write!(f, ", {}", StringRepr(name))?;
        }
        f.write_char(')')?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let tz = self.get(vm.heap);
        let s = tzname_string(tz.offset_seconds, tz.name.as_deref());
        Ok(allocate_string(s, vm.heap))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        // Each method takes the `dt` it would need to resolve a DST rule. A fixed
        // offset has no such rule, so `take_dt_arg` validates and discards it.
        match attr.string_id() {
            Some(id) if id == StaticStrings::Utcoffset => {
                take_dt_arg("timezone.utcoffset", args, vm)?;
                let offset_seconds = self.get(vm.heap).offset_seconds;
                Ok(CallResult::Value(utcoffset_value(Some(offset_seconds), vm.heap)))
            }
            Some(id) if id == StaticStrings::Tzname => {
                take_dt_arg("timezone.tzname", args, vm)?;
                let tz = self.get(vm.heap);
                let name = tzname_string(tz.offset_seconds, tz.name.as_deref());
                Ok(CallResult::Value(allocate_string(name, vm.heap)))
            }
            Some(id) if id == StaticStrings::Dst => {
                take_dt_arg("timezone.dst", args, vm)?;
                // A fixed offset never observes daylight saving.
                Ok(CallResult::Value(Value::None))
            }
            _ => Err(ExcType::attribute_error_method(Type::TimeZone, attr, args, vm)),
        }
    }
}

/// Consumes the single `dt` argument shared by `utcoffset` / `tzname` / `dst`.
///
/// CPython qualifies the arity error (`timezone.dst() takes exactly one
/// argument`) but names the bare method in the type error, so the qualified
/// name is split apart here rather than passed as two literals per call site.
fn take_dt_arg(qualified_name: &'static str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<()> {
    let dt = args.get_one_arg(qualified_name, vm.heap)?;
    defer_drop!(dt, vm);
    let (_, method) = qualified_name
        .split_once('.')
        .expect("tzinfo method names are qualified as `timezone.<method>`");
    check_tzinfo_dt_arg(method, dt, vm.heap, vm.interns)
}
