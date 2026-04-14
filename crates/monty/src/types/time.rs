//! Python `datetime.time` implementation.
//!
//! Monty stores times as `(hour, minute, second, microsecond, fold, tzinfo)` and
//! layers CPython-compatible constructor validation, repr/str formatting, and
//! comparison semantics. Like `datetime.datetime`, an optional `tzinfo` may be
//! attached as a `TimeZone` instance — CPython's full `tzinfo` ABC is not
//! supported in phase 1, matching the existing `datetime.datetime` scope.
//!
//! This is phase-1 only: the full CPython surface (`replace`, `strftime`,
//! `fromisoformat`, `utcoffset`, `tzname`, `dst`, `__format__`) is not yet
//! implemented. Minimum viable surface:
//! - Constructor with validation and `tzinfo`/`fold`
//! - Attribute access: `hour`, `minute`, `second`, `microsecond`, `tzinfo`, `fold`
//! - `__eq__`, `__lt__`/`__le__`/`__gt__`/`__ge__`, `__hash__`, `__repr__`, `__str__`
//! - `isoformat()`
//!
//! Like `datetime.datetime`, aware and naive time values never compare equal
//! (they also cannot be ordered against each other). Two aware times with the
//! same offset are compared by their local clock fields, *not* by shifting to
//! UTC, because a bare time has no date to shift against — matching CPython.

use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::{
        PyTrait, TimeZone, Type,
        str::{Str, StringRepr},
        timezone, value_to_i32,
    },
    value::{EitherStr, Value},
};

/// `datetime.time` storage.
///
/// Time fields are kept in the narrowest integer widths that can represent the
/// validated ranges, keeping the struct compact. `offset_seconds` + `timezone_name`
/// mirror `DateTime`'s fixed-offset tzinfo representation, and `tzinfo_ref`
/// preserves the original `tzinfo` object identity so `t.tzinfo is input_tz`
/// works across attribute access, matching CPython.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Time {
    /// Hour in `0..=23`.
    hour: u8,
    /// Minute in `0..=59`.
    minute: u8,
    /// Second in `0..=59`.
    second: u8,
    /// Microsecond in `0..=999_999`.
    microsecond: u32,
    /// Fold flag: 0 or 1. Used by CPython to disambiguate repeated wall clocks
    /// during DST fall-back transitions; Monty stores it for round-trip fidelity
    /// but does not interpret it.
    fold: u8,
    /// Fixed UTC offset seconds for aware times (`None` for naive).
    offset_seconds: Option<i32>,
    /// Optional display name for the attached timezone.
    timezone_name: Option<String>,
    /// Stable tzinfo object identity for aware times.
    ///
    /// CPython preserves `t.tzinfo is input_tz` and repeated `t.tzinfo` access
    /// returns the same object. We store a retained heap reference so attribute
    /// lookup returns a stable object instead of allocating each time.
    #[serde(default)]
    tzinfo_ref: Option<HeapId>,
}

impl PartialEq for Time {
    fn eq(&self, other: &Self) -> bool {
        // Like CPython, aware and naive times never compare equal. For aware
        // pairs we compare by offset-adjusted microseconds — but because `time`
        // has no date, this is equivalent to comparing local-clock fields when
        // the offsets match. CPython does normalize by subtracting offsets, so
        // we do the same here to handle two aware times with different offsets
        // that represent the same wall-clock instant.
        if self.offset_seconds.is_some() != other.offset_seconds.is_some() {
            return false;
        }
        micros_of_day_adjusted(self) == micros_of_day_adjusted(other)
    }
}

impl Eq for Time {}

impl Hash for Time {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash must be consistent with equality: aware times hash by
        // offset-adjusted microseconds; naive times hash by local microseconds.
        // Whether the time is aware is mixed in so that aware/naive with the
        // same numeric micros still get different hashes (though equality
        // already rules them out).
        self.offset_seconds.is_some().hash(state);
        micros_of_day_adjusted(self).hash(state);
    }
}

/// Returns the microsecond-of-day minus the offset (if aware) as an `i64`.
///
/// Used by equality and hashing so two aware times with different offsets but
/// the same UTC-equivalent wall clock compare equal. Uses `i64` to comfortably
/// hold 86_400_000_000 ± 86399 * 1_000_000.
fn micros_of_day_adjusted(time: &Time) -> i64 {
    let local = i64::from(time.hour) * 3_600_000_000
        + i64::from(time.minute) * 60_000_000
        + i64::from(time.second) * 1_000_000
        + i64::from(time.microsecond);
    match time.offset_seconds {
        Some(offset) => local - i64::from(offset) * 1_000_000,
        None => local,
    }
}

/// Constructor for `time(hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0)`.
///
/// Matches CPython's argument semantics: all components default to 0, `tzinfo`
/// defaults to `None` and may be `None` or a `TimeZone` instance, and `fold` is
/// keyword-only with a default of 0 (must be 0 or 1). Raises `ValueError` when
/// any component is out of range, and `TypeError` for unknown keywords or a
/// non-tzinfo value for `tzinfo`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);
    // Keep the provided tzinfo object alive across argument parsing so we can
    // safely retain its identity in the constructed time.
    let retained_tzinfo = Value::None;
    defer_drop_mut!(retained_tzinfo, heap);

    let mut hour: i32 = 0;
    let mut minute: i32 = 0;
    let mut second: i32 = 0;
    let mut microsecond: i32 = 0;
    let mut fold: i32 = 0;
    let mut tzinfo: Option<TimeZone> = None;
    let mut tzinfo_ref: Option<HeapId> = None;
    let mut seen_hour = false;
    let mut seen_minute = false;
    let mut seen_second = false;
    let mut seen_microsecond = false;
    let mut seen_tzinfo = false;
    let mut seen_fold = false;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => {
                hour = value_to_i32(arg)?;
                seen_hour = true;
            }
            1 => {
                minute = value_to_i32(arg)?;
                seen_minute = true;
            }
            2 => {
                second = value_to_i32(arg)?;
                seen_second = true;
            }
            3 => {
                microsecond = value_to_i32(arg)?;
                seen_microsecond = true;
            }
            4 => {
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(arg, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
                seen_tzinfo = true;
            }
            _ => {
                return Err(SimpleException::new_msg(
                    ExcType::TypeError,
                    format!("function takes at most 5 positional arguments ({} given)", index + 1),
                )
                .into());
            }
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        match key_name.string_id() {
            Some(id) if id == StaticStrings::Hour => {
                if seen_hour {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "hour", 1));
                }
                hour = value_to_i32(value)?;
                seen_hour = true;
            }
            Some(id) if id == StaticStrings::Minute => {
                if seen_minute {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "minute", 2));
                }
                minute = value_to_i32(value)?;
                seen_minute = true;
            }
            Some(id) if id == StaticStrings::Second => {
                if seen_second {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "second", 3));
                }
                second = value_to_i32(value)?;
                seen_second = true;
            }
            Some(id) if id == StaticStrings::Microsecond => {
                if seen_microsecond {
                    return Err(ExcType::type_error_positional_keyword_conflict(
                        "function",
                        "microsecond",
                        4,
                    ));
                }
                microsecond = value_to_i32(value)?;
                seen_microsecond = true;
            }
            Some(id) if id == StaticStrings::Tzinfo => {
                if seen_tzinfo {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "tzinfo", 5));
                }
                let (value_tzinfo, value_tzinfo_ref) = tzinfo_from_value(value, heap)?;
                update_retained_tzinfo(retained_tzinfo, value_tzinfo_ref, heap);
                tzinfo = value_tzinfo;
                tzinfo_ref = value_tzinfo_ref;
                seen_tzinfo = true;
            }
            Some(id) if id == StaticStrings::Fold => {
                if seen_fold {
                    return Err(ExcType::type_error_positional_keyword_conflict("function", "fold", 6));
                }
                fold = value_to_i32(value)?;
                seen_fold = true;
            }
            _ => {
                return Err(ExcType::type_error_c_unexpected_keyword(key_name.as_str(interns)));
            }
        }
    }

    let time = from_components(hour, minute, second, microsecond, fold, tzinfo, tzinfo_ref, heap)?;
    Ok(Value::Ref(heap.allocate(HeapData::Time(time))?))
}

/// Creates a `Time` from validated civil components and optional tzinfo.
///
/// Validates each component's range with CPython-compatible error messages and
/// allocates a stable `tzinfo_ref` when the provided timezone object isn't
/// preserved verbatim.
#[expect(clippy::too_many_arguments)]
fn from_components(
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    fold: i32,
    tzinfo: Option<TimeZone>,
    tzinfo_ref: Option<HeapId>,
    heap: &mut Heap<impl ResourceTracker>,
) -> RunResult<Time> {
    if !(0..=23).contains(&hour) {
        return Err(SimpleException::new_msg(ExcType::ValueError, format!("hour must be in 0..23, not {hour}")).into());
    }
    if !(0..=59).contains(&minute) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("minute must be in 0..59, not {minute}")).into(),
        );
    }
    if !(0..=59).contains(&second) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("second must be in 0..59, not {second}")).into(),
        );
    }
    if !(0..=999_999).contains(&microsecond) {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!("microsecond must be in 0..999999, not {microsecond}"),
        )
        .into());
    }
    if fold != 0 && fold != 1 {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("fold must be either 0 or 1, not {fold}")).into(),
        );
    }

    let (offset_seconds, timezone_name) = match tzinfo {
        Some(tz) => (Some(tz.offset_seconds), tz.name),
        None => (None, None),
    };

    let mut time = Time {
        hour: u8::try_from(hour).expect("hour validated to 0..=23"),
        minute: u8::try_from(minute).expect("minute validated to 0..=59"),
        second: u8::try_from(second).expect("second validated to 0..=59"),
        microsecond: u32::try_from(microsecond).expect("microsecond validated to 0..=999_999"),
        fold: u8::try_from(fold).expect("fold validated to 0..=1"),
        offset_seconds,
        timezone_name,
        tzinfo_ref: None,
    };

    attach_or_allocate_tzinfo_ref(&mut time, tzinfo_ref, heap)?;
    Ok(time)
}

/// Validates a constructor tzinfo argument and extracts timezone data.
///
/// Returns `(None, None)` for `Value::None` and `(Some(TimeZone), Some(id))` for
/// a `TimeZone` heap value. Rejects all other types with `TypeError` matching
/// CPython's wording. `time` does not yet support the full `tzinfo` ABC — only
/// the built-in `timezone` class — mirroring the existing `datetime` scope.
fn tzinfo_from_value(
    value: &Value,
    heap: &Heap<impl ResourceTracker>,
) -> RunResult<(Option<TimeZone>, Option<HeapId>)> {
    match value {
        Value::None => Ok((None, None)),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::TimeZone(tz) => Ok((Some(tz.clone()), Some(*id))),
            other => Err(ExcType::type_error_tzinfo(other.py_type())),
        },
        _ => Err(ExcType::type_error_tzinfo(value.py_type_shallow())),
    }
}

/// Keeps a tzinfo heap value alive during constructor arg parsing.
///
/// Matches the pattern used by `datetime.rs`: before each argument is dropped
/// we inc-ref the currently selected tzinfo so it survives defer_drop of the
/// original argument, and can be inc-ref'd again when attached to the result.
fn update_retained_tzinfo(
    retained_tzinfo: &mut Value,
    tzinfo_ref: Option<HeapId>,
    heap: &mut Heap<impl ResourceTracker>,
) {
    let old = mem::replace(retained_tzinfo, Value::None);
    old.drop_with_heap(heap);
    *retained_tzinfo = if let Some(tzinfo_ref) = tzinfo_ref {
        heap.inc_ref(tzinfo_ref);
        Value::Ref(tzinfo_ref)
    } else {
        Value::None
    };
}

/// Attaches a stable tzinfo identity to the aware time, preserving the original
/// object identity when one was provided so `t.tzinfo is input_tz` holds.
fn attach_or_allocate_tzinfo_ref(
    time: &mut Time,
    preferred_tzinfo_ref: Option<HeapId>,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<(), ResourceError> {
    let Some(offset_seconds) = time.offset_seconds else {
        time.tzinfo_ref = None;
        return Ok(());
    };

    let tzinfo_ref = if let Some(tzinfo_ref) = preferred_tzinfo_ref {
        heap.inc_ref(tzinfo_ref);
        tzinfo_ref
    } else {
        allocate_tzinfo_ref(offset_seconds, time.timezone_name.clone(), heap)?
    };
    time.tzinfo_ref = Some(tzinfo_ref);
    Ok(())
}

/// Allocates a timezone object for time storage, canonicalizing UTC to the
/// shared singleton object.
fn allocate_tzinfo_ref(
    offset_seconds: i32,
    timezone_name: Option<String>,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<HeapId, ResourceError> {
    if offset_seconds == 0 && timezone_name.is_none() {
        let utc = heap.get_timezone_utc()?;
        defer_drop!(utc, heap);
        let Value::Ref(id) = utc else {
            unreachable!("timezone.utc must be heap-allocated");
        };
        heap.inc_ref(*id);
        return Ok(*id);
    }
    let tz = TimeZone {
        offset_seconds,
        name: timezone_name,
    };
    heap.allocate(HeapData::TimeZone(tz))
}

/// Formats a `Time` as `HH:MM:SS[.ffffff][±HH:MM]`, matching CPython's
/// `time.isoformat()`/`str(time)`.
fn format_isoformat(time: &Time) -> String {
    let mut s = format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second);
    if time.microsecond != 0 {
        write!(s, ".{:06}", time.microsecond).expect("writing to String cannot fail");
    }
    if let Some(offset) = time.offset_seconds {
        s.push_str(&timezone::format_offset_hms(offset));
    }
    s
}

impl HeapItem for Time {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.timezone_name.as_ref().map_or(0, String::len)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(tzinfo_ref) = self.tzinfo_ref {
            stack.push(tzinfo_ref);
        }
    }
}

/// `HeapRead`-based dispatch for `Time`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident times.
impl<'h> PyTrait<'h> for HeapRead<'h, Time> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::Time
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(self.get(vm.heap) == other.get(vm.heap))
    }

    fn py_cmp(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<Ordering>, ResourceError> {
        let a = self.get(vm.heap);
        let b = other.get(vm.heap);
        // CPython refuses to compare aware vs naive times. Returning None here
        // propagates as `NotImplemented` in py_cmp callers.
        if a.offset_seconds.is_some() != b.offset_seconds.is_some() {
            return Ok(None);
        }
        Ok(micros_of_day_adjusted(a).partial_cmp(&micros_of_day_adjusted(b)))
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        // Unlike `timedelta`, a `datetime.time` is always truthy — even
        // `time(0, 0)` — matching CPython.
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let t = self.get(vm.heap);
        // CPython omits trailing zero `second`/`microsecond` fields for a cleaner
        // repr; `minute` is always printed so the shortest form is e.g.
        // `datetime.time(0, 0)`.
        write!(f, "datetime.time({}, {}", t.hour, t.minute)?;
        if t.second != 0 || t.microsecond != 0 {
            write!(f, ", {}", t.second)?;
        }
        if t.microsecond != 0 {
            write!(f, ", {}", t.microsecond)?;
        }
        if let Some(offset) = t.offset_seconds {
            if offset == 0 && t.timezone_name.is_none() {
                f.write_str(", tzinfo=datetime.timezone.utc")?;
            } else {
                let timedelta_repr = timezone::format_offset_timedelta_repr(offset);
                write!(f, ", tzinfo=datetime.timezone({timedelta_repr}")?;
                if let Some(name) = &t.timezone_name {
                    write!(f, ", {}", StringRepr(name))?;
                }
                f.write_char(')')?;
            }
        }
        if t.fold != 0 {
            write!(f, ", fold={}", t.fold)?;
        }
        f.write_char(')')?;
        Ok(())
    }

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        Ok(Cow::Owned(format_isoformat(self.get(vm.heap))))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        // Clone so the HeapRead borrow is released before we may allocate a
        // string to return.
        let t = self.get(vm.heap).clone();
        match attr.string_id() {
            Some(id) if id == StaticStrings::Isoformat => {
                args.check_zero_args("time.isoformat", vm.heap)?;
                let s = format_isoformat(&t);
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Str(Str::new(s)))?,
                )))
            }
            _ => Err(ExcType::attribute_error(Type::Time, attr.as_str(vm.interns))),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        // Clone so the HeapRead borrow is released before we might allocate a
        // timezone value for the `tzinfo` attribute.
        let t = self.get(vm.heap).clone();
        match attr.string_id() {
            Some(id) if id == StaticStrings::Hour => Ok(Some(CallResult::Value(Value::Int(i64::from(t.hour))))),
            Some(id) if id == StaticStrings::Minute => Ok(Some(CallResult::Value(Value::Int(i64::from(t.minute))))),
            Some(id) if id == StaticStrings::Second => Ok(Some(CallResult::Value(Value::Int(i64::from(t.second))))),
            Some(id) if id == StaticStrings::Microsecond => {
                Ok(Some(CallResult::Value(Value::Int(i64::from(t.microsecond)))))
            }
            Some(id) if id == StaticStrings::Fold => Ok(Some(CallResult::Value(Value::Int(i64::from(t.fold))))),
            Some(id) if id == StaticStrings::Tzinfo => {
                if let Some(tzinfo_ref) = t.tzinfo_ref {
                    vm.heap.inc_ref(tzinfo_ref);
                    return Ok(Some(CallResult::Value(Value::Ref(tzinfo_ref))));
                }
                if let Some(offset_seconds) = t.offset_seconds {
                    if offset_seconds == 0 && t.timezone_name.is_none() {
                        return Ok(Some(CallResult::Value(vm.heap.get_timezone_utc()?)));
                    }
                    let tz = TimeZone {
                        offset_seconds,
                        name: t.timezone_name,
                    };
                    return Ok(Some(CallResult::Value(Value::Ref(
                        vm.heap.allocate(HeapData::TimeZone(tz))?,
                    ))));
                }
                Ok(Some(CallResult::Value(Value::None)))
            }
            _ => Ok(None),
        }
    }
}
