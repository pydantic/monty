//! Python `datetime.time` implementation.
//!
//! A `time` is a wall clock with no date attached: hour, minute, second,
//! microsecond, an optional fixed-offset `tzinfo`, and a `fold` flag. Each field is
//! stored in the narrowest integer that holds its validated range. As with
//! `datetime.datetime`, only the built-in `timezone` class is accepted as `tzinfo`;
//! CPython's `tzinfo` ABC is not implemented.
//!
//! Aware and naive times never compare equal, and cannot be ordered against each
//! other. Two aware times compare by offset-adjusted microseconds, which are not
//! wrapped into a 24-hour day: a bare time has no date to carry into, so
//! `time(1, 0, tzinfo=utc)` differs from `time(23, 0, tzinfo=minus_two)`. CPython
//! does the same.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use chrono::{NaiveDate, NaiveTime, format::StrftimeItems};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    hash::HashValue,
    heap::{Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput, HeapReader},
    intern::StaticStrings,
    types::{
        CmpOrder, LazyHeapSet, PyTrait, TimeZone, Type,
        date::{self, StrftimeArgs},
        datetime::{allocate_tzinfo_ref, tzinfo_from_value},
        str::{StringRepr, allocate_string, allocate_string_no_interning},
        timezone,
    },
    value::{EitherStr, Value},
};

/// `datetime.time` storage.
///
/// `tzinfo` is `Some` exactly when the time is aware, so an aware time without an
/// attached timezone object cannot be represented. Repr, `py_getattr` and GC
/// traversal therefore have no such case to handle.
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
    /// Fold flag, 0 or 1. CPython uses it to disambiguate the repeated wall clock
    /// of a DST fall-back; Monty has no DST model, so it round-trips the value
    /// without interpreting it (as CPython does, it is excluded from `==`/`hash`).
    fold: u8,
    /// Owned heap reference to the attached `timezone`, or `None` for a naive
    /// time.
    ///
    /// The referenced object is the *only* copy of the offset and name, so the
    /// two cannot drift apart; read it with [`attached_timezone`] or
    /// [`attached_offset`]. It also gives `t.tzinfo is input_tz`. Released and
    /// traced through [`Time::tzinfo_ref`].
    tzinfo: Option<HeapId>,
}

impl Time {
    /// Returns the retained `tzinfo` heap reference for aware times.
    ///
    /// `for_each_child_id` and `py_dec_ref_ids_for_data` MUST both report this id:
    /// omitting it from the cascade leaks the timezone, omitting it from traversal
    /// frees it early.
    pub(crate) fn tzinfo_ref(&self) -> Option<HeapId> {
        self.tzinfo
    }

    /// The wall-clock components and `fold`, for a value crossing to the host.
    ///
    /// Unlike the `datetime` equivalent this cannot fail: every field is bounded
    /// by its own type, and there is no date to fall out of range.
    pub(crate) fn to_components(&self) -> (u8, u8, u8, u32, u8) {
        (self.hour, self.minute, self.second, self.microsecond, self.fold)
    }

    /// Whether every stored component is inside the range [`from_components`]
    /// enforces.
    ///
    /// Deserializing writes these fields directly, so `Heap`'s restore pass
    /// re-checks them: [`naive_time`] treats the ranges as established by
    /// construction, and a forged dump carrying `hour = 255` would panic there
    /// the first time the restored value reached `strftime()`. The offset is not
    /// checked here — it lives on the referenced `timezone`, which restore
    /// range-checks once for every referrer.
    pub(crate) fn components_in_range(&self) -> bool {
        self.hour <= 23 && self.minute <= 59 && self.second <= 59 && self.microsecond <= 999_999 && self.fold <= 1
    }
}

/// The attached timezone of an aware time, cloned from the heap.
///
/// The heap identity is *not* included; pair this with [`Time::tzinfo_ref`] when
/// the caller needs to keep `is` identity.
pub(crate) fn attached_timezone(time: &Time, heap: &HeapReader<'_>) -> Option<TimeZone> {
    let tz_id = time.tzinfo?;
    match heap.read(tz_id) {
        HeapReadOutput::TimeZone(tz) => Some(tz.get(heap).clone()),
        // Constructors only ever attach a `timezone`, and restore rejects a dump
        // whose reference lands anywhere else.
        _ => unreachable!("a time's tzinfo reference always points at a timezone"),
    }
}

/// The UTC offset of an aware time, in seconds.
///
/// [`attached_timezone`] without the name's `String` clone, for the comparison
/// and formatting paths that only need the offset.
fn attached_offset(time: &Time, heap: &HeapReader<'_>) -> Option<i32> {
    let tz_id = time.tzinfo?;
    match heap.read(tz_id) {
        HeapReadOutput::TimeZone(tz) => Some(tz.get(heap).offset_seconds),
        _ => unreachable!("a time's tzinfo reference always points at a timezone"),
    }
}

impl Time {
    /// Microsecond of day, minus the UTC offset for aware times.
    ///
    /// The one key used for both ordering and equality: subtracting the offset makes
    /// two aware times with different offsets but the same UTC clock compare equal.
    /// Not wrapped into a 24-hour day; see the module docs.
    ///
    /// The offset is passed in rather than read here because it lives on the
    /// referenced `timezone`; this is why `Time` has no `PartialEq`/`Hash` of its
    /// own, and `py_eq_impl`, `py_hash` and `py_cmp` share this instead.
    fn adjusted_micros(&self, offset_seconds: Option<i32>) -> i64 {
        let local = i64::from(self.hour) * 3_600_000_000
            + i64::from(self.minute) * 60_000_000
            + i64::from(self.second) * 1_000_000
            + i64::from(self.microsecond);
        match offset_seconds {
            Some(offset_seconds) => local - i64::from(offset_seconds) * 1_000_000,
            None => local,
        }
    }
}

/// Constructor for `time(hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0)`.
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let TimeInitArgs {
        hour,
        minute,
        second,
        microsecond,
        tzinfo,
        fold,
    } = TimeInitArgs::from_args(args, vm)?;
    // `tzinfo` owns the input ref; hold it until `attach_tzinfo` has taken its
    // own reference, or the TimeZone could be freed out from under us.
    defer_drop_mut!(tzinfo, vm);

    allocate(vm, hour, minute, second, microsecond, fold, tzinfo)
}

/// Validates the components and allocates the `time`, taking a reference to
/// `tzinfo` if one is attached.
///
/// `tzinfo` is borrowed: the caller keeps its own reference, and this takes another
/// if it attaches one. Shared with `datetime.time()` / `datetime.timetz()`, so a
/// time built from a datetime is validated exactly like the constructor's.
pub(crate) fn allocate(
    vm: &mut VM<'_>,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    fold: i32,
    tzinfo: &Value,
) -> RunResult<Value> {
    // CPython's `check_time_args` validates every numeric field, `fold` included,
    // before `check_tzinfo_subclass` runs, so `time(25, tzinfo='x')` reports the
    // hour rather than the tzinfo.
    let time = from_components(hour, minute, second, microsecond, fold)?;
    // Only the reference is kept: the `timezone` object itself carries the offset
    // and name, so `tzinfo_from_value`'s decoded copy is used for validation only.
    let (_, tz_ref) = tzinfo_from_value(tzinfo, vm.heap, vm.interns)?;
    Ok(allocate_with_tz(vm, time, tz_ref))
}

/// Attaches an already-resolved timezone to a validated `Time` and allocates it.
///
/// `tz_ref` is borrowed: this takes its own reference, so the caller must keep
/// the original alive across the call. Split out from [`allocate`] for
/// `replace()`, which carries the existing zone over without a `Value` to
/// re-resolve (and must not fabricate a borrowed `Value::Ref`, which the
/// reference-counting checks reject).
fn allocate_with_tz(vm: &mut VM<'_>, mut time: Time, tz_ref: Option<HeapId>) -> Value {
    if let Some(tz_ref) = tz_ref {
        vm.heap.inc_ref(tz_ref);
        time.tzinfo = Some(tz_ref);
    }
    Value::Ref(vm.heap.allocate(HeapData::Time(time)))
}

/// `time.fromisoformat(s)` — the inverse of `isoformat()`.
///
/// Parsing is delegated to `speedate`, the same parser `date`/`datetime` use,
/// so the accepted grammar (and the rejection message) stays consistent across
/// the three classes.
pub(crate) fn class_fromisoformat(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let value = args.get_one_arg("time.fromisoformat", vm.heap)?;
    let s = date::extract_str_arg(&value, "fromisoformat", vm.heap, vm.interns);
    value.drop_with(vm.heap);
    let s = s?;

    let parsed = speedate::Time::parse_str(&s)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, format!("Invalid isoformat string: '{s}'")))?;
    let mut time = from_components(
        i32::from(parsed.hour),
        i32::from(parsed.minute),
        i32::from(parsed.second),
        i32::try_from(parsed.microsecond).expect("speedate microseconds are in 0..999_999"),
        0,
    )?;

    if let Some(offset_seconds) = parsed.tz_offset {
        // Validates the offset bounds with CPython's wording before we commit to
        // an aware time; `allocate_tzinfo_ref` then hands back an owned
        // reference (the `timezone.utc` singleton for a `Z`/`+00:00` suffix).
        TimeZone::new(offset_seconds, None)?;
        time.tzinfo = Some(allocate_tzinfo_ref(offset_seconds, None, vm.heap));
    }

    Ok(Value::Ref(vm.heap.allocate(HeapData::Time(time))))
}

/// Argument shape for `time(hour=0, minute=0, second=0, microsecond=0,
/// tzinfo=None, *, fold=0)`.
///
/// `style = c` reproduces CPython's two over-arity wordings: a sixth positional
/// blames the positional limit (the overflow could still have filled keyword-only
/// `fold`), a seventh the total. `tzinfo` stays a raw [`Value`] so the
/// None-or-timezone check runs in the body with CPython's wording.
#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
struct TimeInitArgs {
    #[from_args(default = 0)]
    hour: i32,
    #[from_args(default = 0)]
    minute: i32,
    #[from_args(default = 0)]
    second: i32,
    #[from_args(default = 0)]
    microsecond: i32,
    #[from_args(default = Value::None)]
    tzinfo: Value,
    #[from_args(kw_only, default = 0)]
    fold: i32,
}

/// Validates the civil components and builds a naive `Time`.
///
/// Field order matches CPython's `check_time_args`, so the first out-of-range
/// component is the one reported.
fn from_components(hour: i32, minute: i32, second: i32, microsecond: i32, fold: i32) -> RunResult<Time> {
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

    Ok(Time {
        hour: u8::try_from(hour).expect("hour validated to 0..=23"),
        minute: u8::try_from(minute).expect("minute validated to 0..=59"),
        second: u8::try_from(second).expect("second validated to 0..=59"),
        microsecond: u32::try_from(microsecond).expect("microsecond validated to 0..=999_999"),
        fold: u8::try_from(fold).expect("fold validated to 0..=1"),
        tzinfo: None,
    })
}

/// Builds a `Time` from host-supplied components, allocating its `tzinfo`.
///
/// The boundary counterpart of [`allocate`]: the host has no heap object to
/// borrow, so an aware time gets a freshly allocated `timezone` (or the
/// `timezone.utc` singleton) rather than a reference to an existing one.
pub(crate) fn from_boundary_components(
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    fold: i32,
    tzinfo: Option<TimeZone>,
    heap: &mut Heap,
) -> RunResult<Time> {
    let mut time = from_components(hour, minute, second, microsecond, fold)?;
    if let Some(tz) = tzinfo {
        time.tzinfo = Some(allocate_tzinfo_ref(tz.offset_seconds, tz.name, heap));
    }
    Ok(time)
}

/// Formats a `Time` as `HH[:MM[:SS[.fff[fff]]]][±HH:MM[:SS]]` for `isoformat()`
/// and `str()`, at the precision `spec` asks for.
fn format_isoformat(time: &Time, offset_seconds: Option<i32>, spec: TimeSpec) -> String {
    let mut s = String::new();
    spec.write_clock(
        &mut s,
        u32::from(time.hour),
        u32::from(time.minute),
        u32::from(time.second),
        time.microsecond,
    );
    if let Some(offset_seconds) = offset_seconds {
        s.push_str(&timezone::format_offset_hms(offset_seconds));
    }
    s
}

impl HeapItem for Time {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Some(tzinfo_ref) = self.tzinfo_ref() {
            stack.push(tzinfo_ref);
        }
    }
}

/// `HeapObjectRead`-based dispatch for `Time`, letting `HeapReadOutput` delegate
/// How much of the clock `isoformat()` renders — CPython's `timespec` argument.
///
/// [`Auto`](Self::Auto) is the default and is the only variant whose output
/// depends on the value: it drops the fractional part when there is none, which
/// is what keeps `isoformat()` round-tripping through `fromisoformat()`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TimeSpec {
    Auto,
    Hours,
    Minutes,
    Seconds,
    Milliseconds,
    Microseconds,
}

impl TimeSpec {
    /// Parses the `timespec` argument.
    ///
    /// CPython names neither the argument nor the offending value in the error,
    /// so neither does this.
    pub(crate) fn parse(spec: &str) -> RunResult<Self> {
        match spec {
            "auto" => Ok(Self::Auto),
            "hours" => Ok(Self::Hours),
            "minutes" => Ok(Self::Minutes),
            "seconds" => Ok(Self::Seconds),
            "milliseconds" => Ok(Self::Milliseconds),
            "microseconds" => Ok(Self::Microseconds),
            _ => Err(SimpleException::new_msg(ExcType::ValueError, "Unknown timespec value").into()),
        }
    }

    /// Appends the clock at this precision. Sub-second digits are truncated
    /// rather than rounded, as in CPython: `.999999` renders as `.999` under
    /// `milliseconds`.
    pub(crate) fn write_clock(self, out: &mut String, hour: u32, minute: u32, second: u32, microsecond: u32) {
        let resolved = match self {
            Self::Auto if microsecond == 0 => Self::Seconds,
            Self::Auto => Self::Microseconds,
            other => other,
        };
        write!(out, "{hour:02}").expect("writing to String cannot fail");
        if matches!(resolved, Self::Hours) {
            return;
        }
        write!(out, ":{minute:02}").expect("writing to String cannot fail");
        if matches!(resolved, Self::Minutes) {
            return;
        }
        write!(out, ":{second:02}").expect("writing to String cannot fail");
        match resolved {
            Self::Milliseconds => write!(out, ".{:03}", microsecond / 1_000),
            Self::Microseconds => write!(out, ".{microsecond:06}"),
            _ => Ok(()),
        }
        .expect("writing to String cannot fail");
    }
}

/// Argument shape for `time.isoformat(timespec='auto')`.
///
/// CPython implements it with Argument Clinic and reports the bare method name,
/// the same shape [`StrftimeArgs`] has — including `bad_arg`, which supplies
/// `isoformat() argument 1 must be str, not int`.
#[derive(FromArgs)]
#[from_args(name = "isoformat", style = c_named, at_most_total, bad_arg)]
struct IsoformatArgs {
    #[from_args(default)]
    timespec: Option<StrArg>,
}

/// Formats a `Time` with a `strftime` directive string, shared by
/// `time.strftime()` and f-string formatting (`f"{t:%H:%M}"`).
///
/// A bare time has no date, so the components are anchored to 1900-01-01, the
/// same anchor CPython's C implementation uses: `time(12, 30).strftime('%Y')`
/// yields `'1900'` on both. As for `datetime`, the naive wall clock is
/// formatted, so `%z`/`%Z` raise rather than emitting an aware time's offset.
pub(crate) fn format_time_strftime(time: &Time, format: &str) -> RunResult<String> {
    let anchored = NaiveDate::from_ymd_opt(1900, 1, 1)
        .expect("1900-01-01 is a valid date")
        .and_time(naive_time(time));
    date::render_strftime(anchored.format_with_items(StrftimeItems::new_lenient(format)))
        .ok_or_else(date::invalid_strftime_error)
}

/// The validated wall-clock components as a `chrono::NaiveTime`.
fn naive_time(time: &Time) -> NaiveTime {
    NaiveTime::from_hms_micro_opt(
        u32::from(time.hour),
        u32::from(time.minute),
        u32::from(time.second),
        time.microsecond,
    )
    .expect("time components are validated on construction")
}

/// `HeapRead`-based dispatch for `Time`, letting `HeapReadOutput` delegate
/// `PyTrait` calls to heap-resident times.
impl<'h> PyTrait<'h> for HeapObjectRead<'h, Time> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Time
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::Time(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let (a, b) = (self.get(vm.heap), other.get(vm.heap));
        // Aware and naive times never compare equal in CPython, whatever the fields.
        Ok(Some(
            a.tzinfo.is_some() == b.tzinfo.is_some()
                && a.adjusted_micros(attached_offset(a, vm.heap)) == b.adjusted_micros(attached_offset(b, vm.heap)),
        ))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let time = self.get(vm.heap);
        let mut hasher = DefaultHasher::new();
        // Must agree with `py_eq_impl`: awareness participates so an aware and a
        // naive time with the same adjusted micros land in different buckets.
        time.tzinfo.is_some().hash(&mut hasher);
        time.adjusted_micros(attached_offset(time, vm.heap)).hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        let (a, b) = (self.get(vm.heap), other.get(vm.heap));
        if a.tzinfo.is_some() != b.tzinfo.is_some() {
            // CPython raises `TypeError` rather than ordering aware against naive.
            return Ok(CmpOrder::Incomparable);
        }
        let (a_micros, b_micros) = (
            a.adjusted_micros(attached_offset(a, vm.heap)),
            b.adjusted_micros(attached_offset(b, vm.heap)),
        );
        Ok(CmpOrder::Ordered(a_micros.cmp(&b_micros)))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        // Every `time` is truthy, midnight included; only `timedelta(0)` is falsy.
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let t = self.get(vm.heap);
        // CPython drops trailing zero `second`/`microsecond`, but always prints
        // `minute`, so the shortest form is `datetime.time(0, 0)`.
        write!(f, "datetime.time({}, {}", t.hour, t.minute)?;
        if t.second != 0 || t.microsecond != 0 {
            write!(f, ", {}", t.second)?;
        }
        if t.microsecond != 0 {
            write!(f, ", {}", t.microsecond)?;
        }
        if let Some(tz) = attached_timezone(t, vm.heap) {
            if tz.offset_seconds == 0 && tz.name.is_none() {
                f.write_str(", tzinfo=datetime.timezone.utc")?;
            } else {
                let timedelta_repr = timezone::format_offset_timedelta_repr(tz.offset_seconds);
                write!(f, ", tzinfo=datetime.timezone({timedelta_repr}")?;
                if let Some(name) = &tz.name {
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

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        let time = self.get(vm.heap);
        let s = format_isoformat(time, attached_offset(time, vm.heap), TimeSpec::Auto);
        Ok(allocate_string_no_interning(s, vm.heap))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        match attr.string_id() {
            Some(id) if id == StaticStrings::Isoformat => {
                let IsoformatArgs { timespec } = IsoformatArgs::from_args(args, vm)?;
                defer_drop!(timespec, vm);
                let spec = match timespec {
                    Some(timespec) => TimeSpec::parse(timespec.as_str(vm))?,
                    None => TimeSpec::Auto,
                };
                let time = self.get(vm.heap);
                let s = format_isoformat(time, attached_offset(time, vm.heap), spec);
                Ok(CallResult::Value(allocate_string_no_interning(s, vm.heap)))
            }
            Some(id) if id == StaticStrings::Strftime => {
                let StrftimeArgs { format } = StrftimeArgs::from_args(args, vm)?;
                defer_drop!(format, vm);
                // Cloned so the heap borrow ends before `format.as_str(vm)`.
                let time = self.get(vm.heap).clone();
                let formatted = format_time_strftime(&time, format.as_str(vm))?;
                Ok(CallResult::Value(allocate_string(formatted, vm.heap)))
            }
            Some(id) if id == StaticStrings::Replace => self.replace(vm, args).map(CallResult::Value),
            Some(id) if id == StaticStrings::Utcoffset => {
                args.check_zero_args("time.utcoffset", vm.heap)?;
                let offset_seconds = attached_offset(self.get(vm.heap), vm.heap);
                Ok(CallResult::Value(timezone::utcoffset_value(offset_seconds, vm.heap)))
            }
            Some(id) if id == StaticStrings::Tzname => {
                args.check_zero_args("time.tzname", vm.heap)?;
                let Some(tz) = attached_timezone(self.get(vm.heap), vm.heap) else {
                    return Ok(CallResult::Value(Value::None));
                };
                let name = timezone::tzname_string(tz.offset_seconds, tz.name.as_deref());
                Ok(CallResult::Value(allocate_string(name, vm.heap)))
            }
            Some(id) if id == StaticStrings::Dst => {
                args.check_zero_args("time.dst", vm.heap)?;
                // Only fixed-offset zones exist, and none of them observes DST.
                Ok(CallResult::Value(Value::None))
            }
            _ => Err(ExcType::attribute_error_method(Type::Time, attr, args, vm)),
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        // Read only the field being asked for: cloning the whole `Time` would
        // copy the timezone name's `String` on every `t.hour`.
        let int_attr = |value: u32| Ok(Some(CallResult::Value(Value::Int(i64::from(value)))));
        match attr.string_id() {
            Some(id) if id == StaticStrings::Hour => int_attr(u32::from(self.get(vm.heap).hour)),
            Some(id) if id == StaticStrings::Minute => int_attr(u32::from(self.get(vm.heap).minute)),
            Some(id) if id == StaticStrings::Second => int_attr(u32::from(self.get(vm.heap).second)),
            Some(id) if id == StaticStrings::Microsecond => int_attr(self.get(vm.heap).microsecond),
            Some(id) if id == StaticStrings::Fold => int_attr(u32::from(self.get(vm.heap).fold)),
            Some(id) if id == StaticStrings::Tzinfo => {
                // `HeapId` is `Copy`, so this ends the heap borrow before `inc_ref`.
                let Some(tzinfo_ref) = self.get(vm.heap).tzinfo_ref() else {
                    return Ok(Some(CallResult::Value(Value::None)));
                };
                vm.heap.inc_ref(tzinfo_ref);
                Ok(Some(CallResult::Value(Value::Ref(tzinfo_ref))))
            }
            _ => Ok(None),
        }
    }
}

/// Time behaviour that needs both the object and the VM, so it cannot be
/// expressed as a plain function over [`Time`].
impl<'h> HeapObjectRead<'h, Time> {
    /// `time.replace(...)` — a copy of `time` with the named components substituted.
    ///
    /// Every field the caller omits is carried over, `fold` and the `tzinfo` object
    /// identity included. Components are validated by [`from_components`], exactly
    /// as the constructor validates them.
    fn replace(&self, vm: &mut VM<'h>, args: ArgValues) -> RunResult<Value> {
        // Components are `Copy` and the zone is taken by value, so the heap borrow
        // ends before `from_args` needs `&mut VM`. The carried-over `tzinfo_ref` is
        // borrowed, kept valid by this read handle until `allocate_with_tz` has
        // taken its own reference.
        let time = self.get(vm.heap);
        let (current_hour, current_minute, current_second) = (time.hour, time.minute, time.second);
        let (current_microsecond, current_fold) = (time.microsecond, time.fold);
        let current_tz_ref = time.tzinfo_ref();

        let TimeReplaceArgs {
            hour,
            minute,
            second,
            microsecond,
            tzinfo,
            fold,
        } = TimeReplaceArgs::from_args(args, vm)?;

        let hour = hour.unwrap_or_else(|| i32::from(current_hour));
        let minute = minute.unwrap_or_else(|| i32::from(current_minute));
        let second = second.unwrap_or_else(|| i32::from(current_second));
        let microsecond = microsecond
            .unwrap_or_else(|| i32::try_from(current_microsecond).expect("microsecond is always in 0..=999_999"));
        let fold = fold.unwrap_or_else(|| i32::from(current_fold));

        match tzinfo {
            // Absent kwarg: carry the current zone over.
            None => {
                let replaced = from_components(hour, minute, second, microsecond, fold)?;
                Ok(allocate_with_tz(vm, replaced, current_tz_ref))
            }
            Some(tzinfo) => {
                defer_drop!(tzinfo, vm);
                allocate(vm, hour, minute, second, microsecond, fold, tzinfo)
            }
        }
    }
}

/// Keyword arguments for `time.replace()`.
///
/// All keyword-only, matching `datetime.replace()` (CPython accepts positionals
/// for both; see limitations/datetime.md). `tzinfo` is an `Option<Value>` so
/// "kwarg absent" (keep the current zone) stays distinct from `tzinfo=None`
/// (make the time naive).
#[derive(FromArgs)]
#[from_args(name = "replace")]
struct TimeReplaceArgs {
    #[from_args(kw_only, default)]
    hour: Option<i32>,
    #[from_args(kw_only, default)]
    minute: Option<i32>,
    #[from_args(kw_only, default)]
    second: Option<i32>,
    #[from_args(kw_only, default)]
    microsecond: Option<i32>,
    #[from_args(kw_only, default)]
    tzinfo: Option<Value>,
    #[from_args(kw_only, default)]
    fold: Option<i32>,
}
