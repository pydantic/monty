//! Python `datetime.timezone` implementation for fixed-offset zones.
//!
//! Phase 1 intentionally supports only fixed offsets (no DST or IANA database).

use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput},
    intern::Interns,
    resource::ResourceTracker,
    types::{
        PyTrait, Type,
        str::StringRepr,
        timedelta,
        timedelta::{MICROSECONDS_PER_SECOND, SECONDS_PER_HOUR, SECONDS_PER_MINUTE},
    },
    value::Value,
};

/// Minimum allowed timezone offset in seconds: -23:59.
pub(crate) const MIN_TIMEZONE_OFFSET_SECONDS: i32 = -86_399;
/// Maximum allowed timezone offset in seconds: +23:59.
pub(crate) const MAX_TIMEZONE_OFFSET_SECONDS: i32 = 86_399;

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
    pub fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        let TimezoneInitArgs { offset, name } = TimezoneInitArgs::from_args(args, vm)?;
        // Keep `offset` and `name` alive across the validation helpers — they
        // own the heap refs (TimeDelta / Str) we're reading from. `name` is
        // an `Option<Value>` so we can distinguish "omitted" from an explicit
        // `None`: CPython accepts `timezone(td)` but rejects `timezone(td,
        // None)` with `TypeError: timezone() argument 2 must be str, not None`.
        defer_drop!(offset, vm);
        let offset_seconds = extract_offset_seconds(offset, vm.heap)?;
        let name_str: Option<String> = match name {
            None => None,
            Some(name) => {
                defer_drop!(name, vm);
                extract_name(name, vm.heap, vm.interns)?
            }
        };

        if offset_seconds == 0 && name_str.is_none() {
            return vm.heap.get_timezone_utc().map_err(Into::into);
        }

        let tz = Self::new(offset_seconds, name_str)?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::TimeZone(tz))?))
    }

    /// Formats offset as `+HH:MM` / `-HH:MM` with optional `:SS`.
    #[must_use]
    pub fn format_utc_offset(&self) -> String {
        format_offset_hms(self.offset_seconds)
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

fn extract_offset_seconds(offset_arg: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<i32> {
    let bad_type = || {
        ExcType::type_error(format!(
            "timezone() argument 1 must be datetime.timedelta, not {}",
            offset_arg.py_type_heap(heap).cpython_arg_name(),
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

/// Formats a canonical `datetime.timedelta(...)` repr for a fixed offset in seconds.
#[must_use]
pub(crate) fn format_offset_timedelta_repr(offset_seconds: i32) -> String {
    let delta = timedelta::from_total_microseconds(i128::from(offset_seconds) * MICROSECONDS_PER_SECOND)
        .expect("timezone offset range is always representable as timedelta");
    timedelta::format_repr(&delta)
}

fn extract_name(name_arg: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Option<String>> {
    match name_arg {
        Value::InternString(id) => Ok(Some(interns.get_str(*id).to_owned())),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => Ok(Some(s.as_str().to_owned())),
            _ => Err(bad_name_arg(name_arg, heap)),
        },
        _ => Err(bad_name_arg(name_arg, heap)),
    }
}

/// Builds the `timezone() argument 2 must be str, not <type>` error CPython
/// raises for any non-`str` `name` argument (including explicit `None`).
fn bad_name_arg(name_arg: &Value, heap: &Heap<impl ResourceTracker>) -> RunError {
    ExcType::type_error(format!(
        "timezone() argument 2 must be str, not {}",
        name_arg.py_type_heap(heap).cpython_arg_name()
    ))
}

impl HeapItem for TimeZone {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.name.as_ref().map_or(0, String::len)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// `HeapRead`-based dispatch for `TimeZone`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident timezone objects.
impl<'h> PyTrait<'h> for HeapRead<'h, TimeZone> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::TimeZone
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::TimeZone(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(
            self.get(vm.heap).offset_seconds == other.get(vm.heap).offset_seconds,
        ))
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
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

    fn py_str(&self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        let tz = self.get(vm.heap);
        if let Some(name) = &tz.name {
            return Ok(Cow::Owned(name.clone()));
        }
        if tz.offset_seconds == 0 {
            return Ok(Cow::Borrowed("UTC"));
        }
        Ok(Cow::Owned(format!("UTC{}", tz.format_utc_offset())))
    }
}
