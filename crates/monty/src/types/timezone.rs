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
    args::ArgValues,
    bytecode::VM,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
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
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let (pos, kwargs) = args.into_parts();
        // CPython's timezone() is C-implemented and counts total args (pos + kwargs).
        // Any total > 2 is rejected before checking individual args.
        let total_args = pos.len() + kwargs.len();
        defer_drop_mut!(pos, heap);
        let kwargs = kwargs.into_iter();
        defer_drop_mut!(kwargs, heap);

        if total_args > 2 {
            return Err(ExcType::type_error_method_at_most("timezone", 2, total_args));
        }

        let mut offset_seconds: Option<i32> = None;
        let mut name: Option<Option<String>> = None;
        let mut seen_offset = false;
        let mut seen_name = false;

        for (index, arg) in pos.by_ref().enumerate() {
            defer_drop!(arg, heap);
            match index {
                0 => {
                    offset_seconds = Some(extract_offset_seconds(arg, heap)?);
                    seen_offset = true;
                }
                1 => {
                    name = Some(extract_name(arg, heap, interns)?);
                    seen_name = true;
                }
                _ => return Err(ExcType::type_error_method_at_most("timezone", 2, index + 1)),
            }
        }

        for (key, value) in kwargs {
            defer_drop!(key, heap);
            defer_drop!(value, heap);

            let Some(key_name) = key.as_either_str(heap) else {
                return Err(ExcType::type_error_kwargs_nonstring_key());
            };
            match key_name.string_id() {
                Some(id) if id == StaticStrings::Offset => {
                    if seen_offset {
                        return Err(ExcType::type_error_positional_keyword_conflict(
                            "timezone()",
                            "offset",
                            1,
                        ));
                    }
                    offset_seconds = Some(extract_offset_seconds(value, heap)?);
                    seen_offset = true;
                }
                Some(id) if id == StaticStrings::Name => {
                    if seen_name {
                        return Err(ExcType::type_error_positional_keyword_conflict("timezone()", "name", 2));
                    }
                    name = Some(extract_name(value, heap, interns)?);
                    seen_name = true;
                }
                _ => {
                    return Err(ExcType::type_error_unexpected_keyword(
                        "timezone",
                        key_name.as_str(interns),
                    ));
                }
            }
        }

        let Some(offset_seconds) = offset_seconds else {
            return Err(ExcType::type_error_c_missing_required_named("timezone", "offset", 1));
        };
        let name = name.unwrap_or(None);
        if offset_seconds == 0 && name.is_none() {
            return heap.get_timezone_utc().map_err(Into::into);
        }

        let tz = Self::new(offset_seconds, name)?;
        Ok(Value::Ref(heap.allocate(HeapData::TimeZone(tz))?))
    }

    /// Formats offset as `+HH:MM` / `-HH:MM` with optional `:SS`.
    #[must_use]
    pub fn format_utc_offset(&self) -> String {
        format_offset_hms(self.offset_seconds)
    }
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
    let Value::Ref(offset_id) = offset_arg else {
        return Err(ExcType::type_error(
            "timezone() argument 1 must be datetime.timedelta".to_owned(),
        ));
    };
    let HeapData::TimeDelta(delta) = heap.get(*offset_id) else {
        return Err(ExcType::type_error(
            "timezone() argument 1 must be datetime.timedelta".to_owned(),
        ));
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
            _ => Err(ExcType::type_error("timezone() argument 2 must be str".to_owned())),
        },
        _ => Err(ExcType::type_error("timezone() argument 2 must be str".to_owned())),
    }
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
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::TimeZone
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(self.get(vm.heap).offset_seconds == other.get(vm.heap).offset_seconds)
    }

    fn py_hash(
        &self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<u64>, ResourceError> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(hasher.finish()))
    }

    fn py_bool(&self, _vm: &mut VM<'h, '_, impl ResourceTracker>) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'h, '_, impl ResourceTracker>,
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

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
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
