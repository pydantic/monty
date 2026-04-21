//! Python `datetime.date` implementation.
//!
//! Monty stores dates with `chrono::NaiveDate` and keeps CPython-compatible
//! constructor validation and arithmetic behavior.

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;
use chrono::{Datelike, NaiveDate};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings},
    os::OsFunction,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, TimeDelta, Type, str::Str, timedelta, value_to_i32},
    value::{EitherStr, Value},
};

const MICROSECONDS_PER_DAY: i128 = 86_400_000_000;

/// `datetime.date` storage backed by `chrono::NaiveDate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Date(pub(crate) NaiveDate);

/// Creates a date from validated civil components.
///
/// Error messages match CPython 3.14 format exactly.
pub(crate) fn from_ymd(year: i32, month: i32, day: i32) -> RunResult<Date> {
    if !(1..=9999).contains(&year) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("year must be in 1..9999, not {year}")).into(),
        );
    }
    if !(1..=12).contains(&month) {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("month must be in 1..12, not {month}")).into(),
        );
    }
    let Ok(month_u32) = u32::try_from(month) else {
        return Err(
            SimpleException::new_msg(ExcType::ValueError, format!("month must be in 1..12, not {month}")).into(),
        );
    };
    let Ok(day_u32) = u32::try_from(day) else {
        return Err(day_out_of_range_error(day, month, year));
    };

    let Some(date) = NaiveDate::from_ymd_opt(year, month_u32, day_u32) else {
        return Err(day_out_of_range_error(day, month, year));
    };
    Ok(Date(date))
}

/// Produces a CPython-compatible error for an invalid day value.
///
/// Format: `"day {day} must be in range 1..{max_day} for month {month} in year {year}"`
fn day_out_of_range_error(day: i32, month: i32, year: i32) -> RunError {
    let max_day = max_day_for_month(year, month);
    SimpleException::new_msg(
        ExcType::ValueError,
        format!("day {day} must be in range 1..{max_day} for month {month} in year {year}"),
    )
    .into()
}

/// Returns the maximum valid day for a given month and year.
fn max_day_for_month(year: i32, month: i32) -> u32 {
    // Try the last possible day (31) and work backwards to find the actual max
    let Ok(month_u32) = u32::try_from(month) else {
        return 31;
    };
    for d in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(year, month_u32, d).is_some() {
            return d;
        }
    }
    31
}

/// Creates a date from a proleptic Gregorian ordinal value.
pub(crate) fn from_ordinal(ordinal: i32) -> RunResult<Date> {
    let Some(date) = NaiveDate::from_num_days_from_ce_opt(ordinal) else {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
    };
    if !(1..=9999).contains(&date.year()) {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
    }
    Ok(Date(date))
}

/// Returns the proleptic Gregorian ordinal (`1 == 0001-01-01`) for a date.
#[must_use]
pub(crate) fn to_ordinal(date: Date) -> i32 {
    date.0.num_days_from_ce()
}

/// Returns civil components `(year, month, day)`.
#[must_use]
pub(crate) fn to_ymd(date: Date) -> (i32, u32, u32) {
    (date.0.year(), date.0.month(), date.0.day())
}

/// Constructor for `date(year, month, day)`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    // CPython's date() is C-implemented and counts total args (pos + kwargs).
    // Any total > 3 is rejected before checking individual args.
    let total_args = pos.len() + kwargs.len();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    if total_args > 3 {
        return Err(ExcType::type_error_c_at_most(3, total_args));
    }

    let mut year: Option<i32> = None;
    let mut month: Option<i32> = None;
    let mut day: Option<i32> = None;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => year = Some(value_to_i32(arg)?),
            1 => month = Some(value_to_i32(arg)?),
            2 => day = Some(value_to_i32(arg)?),
            _ => unreachable!("total_args check above prevents this"),
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);

        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        match key_name.string_id() {
            Some(id) if id == StaticStrings::Year => {
                if year.is_some() {
                    return Err(ExcType::type_error_multiple_values("date", "year"));
                }
                year = Some(value_to_i32(value)?);
            }
            Some(id) if id == StaticStrings::Month => {
                if month.is_some() {
                    return Err(ExcType::type_error_multiple_values("date", "month"));
                }
                month = Some(value_to_i32(value)?);
            }
            Some(id) if id == StaticStrings::Day => {
                if day.is_some() {
                    return Err(ExcType::type_error_multiple_values("date", "day"));
                }
                day = Some(value_to_i32(value)?);
            }
            _ => return Err(ExcType::type_error_unexpected_keyword("date", key_name.as_str(interns))),
        }
    }

    let Some(year) = year else {
        return Err(ExcType::type_error_c_missing_required("year", 1));
    };
    let Some(month) = month else {
        return Err(ExcType::type_error_c_missing_required("month", 2));
    };
    let Some(day) = day else {
        return Err(ExcType::type_error_c_missing_required("day", 3));
    };

    let date = from_ymd(year, month, day)?;
    Ok(Value::Ref(heap.allocate(HeapData::Date(date))?))
}

/// Classmethod implementation for `date.today()`.
///
/// Issues a `DateToday` OS call with no arguments. The host should return
/// `MontyObject::Date` directly.
pub(crate) fn class_today(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
    args.check_zero_args("date.today", heap)?;
    Ok(AttrCallResult::OsCall(OsFunction::DateToday, ArgValues::Empty))
}

/// Classmethod `date.fromisoformat(date_string)`.
///
/// Parses ISO 8601 date strings in the formats `YYYY-MM-DD` and `YYYYMMDD`,
/// matching CPython 3.11+ behavior.
pub(crate) fn class_fromisoformat(
    heap: &mut Heap<impl ResourceTracker>,
    args: ArgValues,
    interns: &Interns,
) -> RunResult<Value> {
    let value = args.get_one_arg("date.fromisoformat", heap)?;
    let s = extract_str_arg(&value, "fromisoformat", heap, interns);
    value.drop_with_heap(heap);
    let s = s?;

    let date = parse_iso_date(&s)
        .ok_or_else(|| SimpleException::new_msg(ExcType::ValueError, format!("Invalid isoformat string: '{s}'")))?;
    Ok(Value::Ref(heap.allocate(HeapData::Date(date))?))
}

/// Parses an ISO 8601 date string into a `Date`.
///
/// Uses speedate for Python-compatible ISO 8601 parsing.
fn parse_iso_date(s: &str) -> Option<Date> {
    let parsed = speedate::Date::parse_bytes(s.as_bytes()).ok()?;
    from_ymd(i32::from(parsed.year), i32::from(parsed.month), i32::from(parsed.day)).ok()
}

/// Extracts a string from a `Value` for use by classmethods.
pub(crate) fn extract_str_arg(
    value: &Value,
    method_name: &str,
    heap: &Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<String> {
    match value {
        Value::InternString(string_id) => Ok(interns.get_str(*string_id).to_owned()),
        Value::Ref(heap_id) => match heap.get(*heap_id) {
            HeapData::Str(s) => Ok(s.as_str().to_owned()),
            _ => Err(ExcType::type_error(format!("{method_name}: argument must be str"))),
        },
        _ => Err(ExcType::type_error(format!("{method_name}: argument must be str"))),
    }
}

impl HeapItem for Date {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}
}

/// `HeapRead`-based dispatch for `Date`, enabling the `HeapReadOutput` enum to
/// delegate `PyTrait` calls to heap-resident dates.
impl<'h> PyTrait<'h> for HeapRead<'h, Date> {
    fn py_type(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Type {
        Type::Date
    }

    fn py_len(&self, _vm: &VM<'h, '_, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(*self.get(vm.heap) == *other.get(vm.heap))
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

    fn py_cmp(
        &self,
        other: &Self,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> Result<Option<Ordering>, ResourceError> {
        Ok(self.get(vm.heap).partial_cmp(other.get(vm.heap)))
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
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        write!(f, "datetime.date({year}, {month}, {day})")?;
        Ok(())
    }

    fn py_str(&self, vm: &VM<'h, '_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        Ok(Cow::Owned(format!("{year:04}-{month:02}-{day:02}")))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let date = *self.get(vm.heap);
        match attr.string_id() {
            Some(id) if id == StaticStrings::Isoformat => {
                args.check_zero_args("date.isoformat", vm.heap)?;
                let (year, month, day) = to_ymd(date);
                Ok(CallResult::Value(Value::Ref(vm.heap.allocate(HeapData::Str(
                    Str::new(format!("{year:04}-{month:02}-{day:02}")),
                ))?)))
            }
            Some(id) if id == StaticStrings::Strftime => {
                let fmt = extract_strftime_arg(args, "date.strftime", vm.heap, vm.interns)?;
                let formatted = date.0.format(&fmt).to_string();
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Str(Str::new(formatted)))?,
                )))
            }
            Some(id) if id == StaticStrings::Replace => {
                let (year, month, day) = to_ymd(date);
                let (new_year, new_month, new_day) =
                    extract_date_replace_kwargs(args, year, month, day, vm.heap, vm.interns)?;
                let new_date = from_ymd(new_year, new_month, new_day)?;
                Ok(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Date(new_date))?,
                )))
            }
            Some(id) if id == StaticStrings::Weekday => {
                args.check_zero_args("date.weekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    date.0.weekday().num_days_from_monday(),
                ))))
            }
            Some(id) if id == StaticStrings::Isoweekday => {
                args.check_zero_args("date.isoweekday", vm.heap)?;
                Ok(CallResult::Value(Value::Int(i64::from(
                    date.0.weekday().number_from_monday(),
                ))))
            }
            _ => Err(ExcType::attribute_error(Type::Date, attr.as_str(vm.interns))),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        match attr.string_id() {
            Some(id) if id == StaticStrings::Year => Ok(Some(CallResult::Value(Value::Int(i64::from(year))))),
            Some(id) if id == StaticStrings::Month => Ok(Some(CallResult::Value(Value::Int(i64::from(month))))),
            Some(id) if id == StaticStrings::Day => Ok(Some(CallResult::Value(Value::Int(i64::from(day))))),
            _ => Ok(None),
        }
    }
}

/// `date - date` returns a timedelta with the difference in days.
pub(crate) fn py_sub_date(
    a: Date,
    b: Date,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let diff_days = i64::from(to_ordinal(a)) - i64::from(to_ordinal(b));
    let Ok(delta) = timedelta::from_total_microseconds(i128::from(diff_days) * MICROSECONDS_PER_DAY) else {
        return Ok(None);
    };
    Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
}

/// `date + timedelta` helper.
pub(crate) fn py_add(
    date: Date,
    delta: TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let (days, _, _) = timedelta::components(&delta);
    let new_ordinal = i64::from(to_ordinal(date)).checked_add(i64::from(days));
    let Some(new_ordinal) = new_ordinal else {
        return Ok(None);
    };
    let Ok(new_ordinal) = i32::try_from(new_ordinal) else {
        return Ok(None);
    };
    match from_ordinal(new_ordinal) {
        Ok(value) => Ok(Some(Value::Ref(heap.allocate(HeapData::Date(value))?))),
        Err(_) => Ok(None),
    }
}

/// `date - timedelta` helper.
pub(crate) fn py_sub_timedelta(
    date: Date,
    delta: TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let (days, _, _) = timedelta::components(&delta);
    let new_ordinal = i64::from(to_ordinal(date)).checked_sub(i64::from(days));
    let Some(new_ordinal) = new_ordinal else {
        return Ok(None);
    };
    let Ok(new_ordinal) = i32::try_from(new_ordinal) else {
        return Ok(None);
    };
    match from_ordinal(new_ordinal) {
        Ok(value) => Ok(Some(Value::Ref(heap.allocate(HeapData::Date(value))?))),
        Err(_) => Ok(None),
    }
}

/// Extracts the format string argument for `strftime()`.
///
/// Accepts exactly one positional string argument.
pub(crate) fn extract_strftime_arg(
    args: ArgValues,
    method_name: &str,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<String> {
    let value = args.get_one_arg(method_name, heap)?;
    let result = match &value {
        Value::InternString(string_id) => Ok(interns.get_str(*string_id).to_owned()),
        Value::Ref(heap_id) => match heap.get(*heap_id) {
            HeapData::Str(s) => Ok(s.as_str().to_owned()),
            _ => Err(ExcType::type_error(
                "descriptor 'strftime' requires a 'str' object but received a non-str type".to_owned(),
            )),
        },
        _ => Err(ExcType::type_error(
            "descriptor 'strftime' requires a 'str' object but received a non-str type".to_owned(),
        )),
    };
    value.drop_with_heap(heap);
    result
}

/// Parses keyword arguments for `date.replace()`.
///
/// Returns `(year, month, day)` with original values as defaults.
fn extract_date_replace_kwargs(
    args: ArgValues,
    year: i32,
    month: u32,
    day: u32,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<(i32, i32, i32)> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let mut new_year = year;
    let mut new_month = i32::try_from(month).expect("month is always in 1..=12");
    let mut new_day = i32::try_from(day).expect("day is always in 1..=31");

    // replace() takes no positional args
    if let Some(arg) = pos.next() {
        arg.drop_with_heap(heap);
        return Err(ExcType::type_error("replace() takes 0 positional arguments".to_owned()));
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        match key_name.string_id() {
            Some(id) if id == StaticStrings::Year => new_year = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Month => new_month = value_to_i32(value)?,
            Some(id) if id == StaticStrings::Day => new_day = value_to_i32(value)?,
            _ => {
                return Err(ExcType::type_error_unexpected_keyword(
                    "replace",
                    key_name.as_str(interns),
                ));
            }
        }
    }

    Ok((new_year, new_month, new_day))
}
