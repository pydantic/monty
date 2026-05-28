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
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunError, RunResult, SimpleException},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapRead},
    intern::{Interns, StaticStrings},
    os::OsFunctionCall,
    resource::{ResourceError, ResourceTracker},
    types::{
        AttrCallResult, PyTrait, TimeDelta, Type,
        str::{allocate_string, allocate_string_no_interning},
        timedelta,
    },
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
pub(crate) fn init(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let DateInitArgs { year, month, day } = DateInitArgs::from_args(args, vm)?;
    let date = from_ymd(year, month, day)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Date(date))?))
}

/// Argument shape for `date(year, month, day)`.
///
/// CPython's `date()` is C-implemented (`PyArg_ParseTupleAndKeywords`) and uses
/// `c_error` wording — "function takes at most N arguments", "function missing
/// required argument 'X' (pos N)", etc. Unlike `datetime()` it does **not**
/// prefix "positional" in the at-most message, so we leave `at_most_positional`
/// unset.
#[derive(FromArgs)]
#[from_args(name = "function", c_error, at_most_total)]
struct DateInitArgs {
    year: i32,
    month: i32,
    day: i32,
}

/// Classmethod implementation for `date.today()`.
///
/// Issues a `DateToday` OS call with no arguments. The host should return
/// `MontyObject::Date` directly.
pub(crate) fn class_today(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
    args.check_zero_args("date.today", heap)?;
    Ok(AttrCallResult::OsCall(OsFunctionCall::DateToday))
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
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Date
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> Result<bool, ResourceError> {
        Ok(*self.get(vm.heap) == *other.get(vm.heap))
    }

    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h, impl ResourceTracker>) -> Result<Option<Ordering>, ResourceError> {
        Ok(self.get(vm.heap).partial_cmp(other.get(vm.heap)))
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
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        write!(f, "datetime.date({year}, {month}, {day})")?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
        let (year, month, day) = to_ymd(*self.get(vm.heap));
        Ok(Cow::Owned(format!("{year:04}-{month:02}-{day:02}")))
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let date = *self.get(vm.heap);
        match attr.string_id() {
            Some(id) if id == StaticStrings::Isoformat => {
                args.check_zero_args("date.isoformat", vm.heap)?;
                let (year, month, day) = to_ymd(date);
                Ok(CallResult::Value(allocate_string_no_interning(
                    format!("{year:04}-{month:02}-{day:02}"),
                    vm.heap,
                )?))
            }
            Some(id) if id == StaticStrings::Strftime => {
                let StrftimeArgs { format } = StrftimeArgs::from_args(args, vm)?;
                let formatted = date.0.format(&format).to_string();
                Ok(CallResult::Value(allocate_string(formatted, vm.heap)?))
            }
            Some(id) if id == StaticStrings::Replace => {
                let (year, month, day) = to_ymd(date);
                let DateReplaceArgs {
                    year: new_year,
                    month: new_month,
                    day: new_day,
                } = DateReplaceArgs::from_args(args, vm)?;
                let new_date = from_ymd(
                    new_year.unwrap_or(year),
                    new_month.unwrap_or(i32::try_from(month).expect("month in 1..=12")),
                    new_day.unwrap_or(i32::try_from(day).expect("day in 1..=31")),
                )?;
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

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<CallResult>> {
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

/// Argument shape for `date.strftime(format)` and `datetime.strftime(format)`.
///
/// CPython implements `strftime` as a C method and reports errors with the
/// bare method name (no class prefix), so we use `c_error_named` + the
/// `"strftime"` descriptor — matching wordings like
/// `strftime() missing required argument 'format' (pos 1)` and
/// `strftime() takes at most 1 argument (2 given)`.
///
/// `bad_arg` opts the wrong-type wording into CPython's `_PyArg_BadArgument`
/// form (`strftime() argument 1 must be str, not <type>`), including the
/// `None`-vs-`NoneType` special case — so the type-check logic lives in
/// the derive rather than a hand-written extract helper.
#[derive(FromArgs)]
#[from_args(name = "strftime", c_error_named, at_most_total, bad_arg)]
pub(crate) struct StrftimeArgs {
    pub(crate) format: String,
}

/// Keyword arguments for `date.replace()`. All keyword-only; absent fields
/// inherit the original date's component via `unwrap_or` at the call site.
#[derive(FromArgs)]
#[from_args(name = "replace")]
struct DateReplaceArgs {
    #[from_args(kw_only, default)]
    year: Option<i32>,
    #[from_args(kw_only, default)]
    month: Option<i32>,
    #[from_args(kw_only, default)]
    day: Option<i32>,
}
