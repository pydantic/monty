//! Implementation of the `time` module.
//!
//! The clocks (`time`, `monotonic`, `perf_counter` and their `_ns` forms) all
//! read the session's `OsPolicy` clock, so a `call_host` host sees one
//! `time.time` call distinguished by its [`TimeCaller`]. `process_time` and
//! `thread_time` follow the separate `process_time` policy, since they exclude
//! sleeps. The conversion functions and the zone constants read the session zone.
//! See `limitations/time.md` for CPython divergences.

use std::{borrow::Cow, time::Duration};

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, Timelike, format::StrftimeItems};
use monty_types::{
    MontyTimeZone, OsFunctionCall, ProcessTime, ResourceTracker, SandboxTimeZone, SleepError, SleepMode, TimeCaller,
    local_wall_clock, sleep_duration, unix_seconds,
};
use num_traits::ToPrimitive;
use smallvec::smallvec;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::PostConversionEffect,
    string_builder::StringBuilder,
    types::{
        Module, NamedTuple, PyTrait, date, datetime as datetime_type,
        datetime::{sandbox_instant, sandbox_local_wall_clock},
        str::allocate_string,
        timezone::tzname_string,
        tuple::allocate_tuple,
    },
    value::{EitherStr, Value},
};

/// `time` module functions, each a Python-visible callable.
///
/// Serialised by declaration index through `ModuleFunctions`, so new variants
/// must be appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum TimeFunctions {
    Time,
    Sleep,
    TimeNs,
    Monotonic,
    MonotonicNs,
    PerfCounter,
    PerfCounterNs,
    ProcessTime,
    ProcessTimeNs,
    ThreadTime,
    ThreadTimeNs,
    Gmtime,
    Localtime,
    Mktime,
    Asctime,
    Ctime,
    Strftime,
    Strptime,
}

/// Every function the module exposes, paired with the name it binds to.
const FUNCTIONS: [(StaticStrings, TimeFunctions); 18] = [
    (StaticStrings::Time, TimeFunctions::Time),
    (StaticStrings::Sleep, TimeFunctions::Sleep),
    (StaticStrings::TimeNs, TimeFunctions::TimeNs),
    (StaticStrings::Monotonic, TimeFunctions::Monotonic),
    (StaticStrings::MonotonicNs, TimeFunctions::MonotonicNs),
    (StaticStrings::PerfCounter, TimeFunctions::PerfCounter),
    (StaticStrings::PerfCounterNs, TimeFunctions::PerfCounterNs),
    (StaticStrings::ProcessTime, TimeFunctions::ProcessTime),
    (StaticStrings::ProcessTimeNs, TimeFunctions::ProcessTimeNs),
    (StaticStrings::ThreadTime, TimeFunctions::ThreadTime),
    (StaticStrings::ThreadTimeNs, TimeFunctions::ThreadTimeNs),
    (StaticStrings::Gmtime, TimeFunctions::Gmtime),
    (StaticStrings::Localtime, TimeFunctions::Localtime),
    (StaticStrings::Mktime, TimeFunctions::Mktime),
    (StaticStrings::Asctime, TimeFunctions::Asctime),
    (StaticStrings::Ctime, TimeFunctions::Ctime),
    (StaticStrings::Strftime, TimeFunctions::Strftime),
    (StaticStrings::Strptime, TimeFunctions::Strptime),
];

/// Creates the `time` module and allocates it on the heap.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Time, vm.interns);
    for (name, function) in FUNCTIONS {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Time(function)), vm);
    }
    set_zone_constants(&mut module, vm);

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Sets `timezone`, `altzone`, `daylight` and `tzname` from the sandbox zone,
/// the values CPython reads from libc for 1 January and 1 July of the current
/// year. A named zone needs that year from the session clock, so the four are
/// unset when the clock is `CallHost`: module creation cannot suspend.
fn set_zone_constants(module: &mut Module, vm: &mut VM<'_>) {
    let year = vm.env.os_policy.datetime.read().map(|utc| utc.year());
    let Some(constants) = vm.env.os_policy.timezone.constants(year) else {
        return;
    };
    // `time.timezone` is seconds *west* of UTC, the opposite sign to `utcoffset()`.
    let west = |zone: &MontyTimeZone| Value::Int(-i64::from(zone.offset_seconds));
    let name = |zone: &MontyTimeZone, vm: &VM<'_>| {
        allocate_string(tzname_string(zone.offset_seconds, zone.name.as_deref()), vm.heap)
    };
    module.set_attr(StaticStrings::Timezone, west(&constants.standard), vm);
    module.set_attr(StaticStrings::Altzone, west(&constants.daylight_zone), vm);
    module.set_attr(StaticStrings::Daylight, Value::Int(i64::from(constants.daylight)), vm);
    let tzname = allocate_tuple(
        smallvec![name(&constants.standard, vm), name(&constants.daylight_zone, vm)],
        vm.heap,
    );
    module.set_attr(StaticStrings::Tzname, tzname, vm);
}

/// Dispatches a call to a `time` module function.
pub(super) fn call(vm: &mut VM<'_>, function: TimeFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        TimeFunctions::Time => clock(vm, args, TimeCaller::Time, None),
        TimeFunctions::Sleep => sleep(vm, args),
        TimeFunctions::TimeNs => clock(vm, args, TimeCaller::TimeNs, Some(ClockReading::Nanoseconds)),
        TimeFunctions::Monotonic => clock(vm, args, TimeCaller::Monotonic, None),
        TimeFunctions::MonotonicNs => clock(vm, args, TimeCaller::MonotonicNs, Some(ClockReading::Nanoseconds)),
        TimeFunctions::PerfCounter => clock(vm, args, TimeCaller::PerfCounter, None),
        TimeFunctions::PerfCounterNs => clock(vm, args, TimeCaller::PerfCounterNs, Some(ClockReading::Nanoseconds)),
        TimeFunctions::ProcessTime | TimeFunctions::ThreadTime => process_time(vm, args, function, false),
        TimeFunctions::ProcessTimeNs | TimeFunctions::ThreadTimeNs => process_time(vm, args, function, true),
        TimeFunctions::Gmtime => convert_clock(vm, args, TimeCaller::Gmtime, ClockReading::Gmtime),
        TimeFunctions::Localtime => convert_clock(vm, args, TimeCaller::Localtime, ClockReading::Localtime),
        TimeFunctions::Mktime => mktime(vm, args),
        TimeFunctions::Asctime => asctime(vm, args),
        TimeFunctions::Ctime => convert_clock(vm, args, TimeCaller::Ctime, ClockReading::Ctime),
        TimeFunctions::Strftime => strftime(vm, args),
        TimeFunctions::Strptime => strptime(vm, args),
    }
}

/// A clock function: returns its reading, or suspends so the host answers it.
/// `reading` is `None` for the plain-seconds clocks (`time`, `monotonic`,
/// `perf_counter`), which need no work on resume. All share the `time.time` OS
/// call; `caller` tells the host which one asked.
fn clock(vm: &mut VM<'_>, args: ArgValues, caller: TimeCaller, reading: Option<ClockReading>) -> RunResult<CallResult> {
    args.check_zero_args(caller.as_str(), vm.heap)?;
    let Some(utc) = sandbox_instant(vm)? else {
        return Ok(suspend_for_clock(caller, reading));
    };
    match reading {
        None => Ok(CallResult::Value(Value::Float(unix_seconds(utc)))),
        Some(reading) => reading.apply(utc, vm).map(CallResult::Value),
    }
}

/// `gmtime`/`localtime`/`ctime`, which take an optional `secs` and fall back to
/// the session clock — the one path where a conversion function can suspend.
fn convert_clock(vm: &mut VM<'_>, args: ArgValues, caller: TimeCaller, reading: ClockReading) -> RunResult<CallResult> {
    // One struct per function because `FromArgs` bakes the name into the
    // binding errors, and CPython reports each function's own.
    let seconds = match caller {
        TimeCaller::Gmtime => GmtimeArgs::from_args(args, vm)?.seconds,
        TimeCaller::Localtime => LocaltimeArgs::from_args(args, vm)?.seconds,
        _ => CtimeArgs::from_args(args, vm)?.seconds,
    };
    let instant = optional_instant(&seconds, vm);
    seconds.drop_with(vm.heap);
    match instant? {
        Some(utc) => reading.apply(utc, vm).map(CallResult::Value),
        None => Ok(suspend_for_clock(caller, Some(reading))),
    }
}

/// `time.gmtime(secs=None)`; `PyArg_ParseTuple` in CPython, hence `parse_tuple`.
/// [`LocaltimeArgs`] and [`CtimeArgs`] are the same shape under their own names.
#[derive(FromArgs)]
#[from_args(name = "gmtime", style = parse_tuple)]
struct GmtimeArgs {
    #[from_args(pos_only, default = Value::None)]
    seconds: Value,
}

/// `time.localtime(secs=None)`; see [`GmtimeArgs`].
#[derive(FromArgs)]
#[from_args(name = "localtime", style = parse_tuple)]
struct LocaltimeArgs {
    #[from_args(pos_only, default = Value::None)]
    seconds: Value,
}

/// `time.ctime(secs=None)`; see [`GmtimeArgs`].
#[derive(FromArgs)]
#[from_args(name = "ctime", style = parse_tuple)]
struct CtimeArgs {
    #[from_args(pos_only, default = Value::None)]
    seconds: Value,
}

/// Suspends with the shared `time.time` call, carrying the work still to do on
/// the host's answer.
fn suspend_for_clock(caller: TimeCaller, reading: Option<ClockReading>) -> CallResult {
    let call = OsFunctionCall::Time(caller);
    match reading {
        None => CallResult::OsCall(call),
        Some(reading) => CallResult::OsCallWithEffect {
            call,
            effect: PostConversionEffect::ClockReading { reading }.into(),
        },
    }
}

/// What a `time` function makes of the instant once the clock has been read.
///
/// Applied inline when the sandbox owns the clock and by [`apply_clock_reading`]
/// when the host answered `time.time`, so the two paths cannot drift.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum ClockReading {
    /// Nanoseconds since the epoch, for the `_ns` clocks.
    Nanoseconds,
    /// `time.gmtime()`: a UTC `struct_time`.
    Gmtime,
    /// `time.localtime()`: a `struct_time` in the session zone.
    Localtime,
    /// `time.ctime()`: `asctime(localtime(secs))`.
    Ctime,
    /// `time.strftime(format)`: the format applied to the local `struct_time`.
    Strftime(Box<str>),
}

impl ClockReading {
    /// Turns the instant into the value the `time` function returns.
    fn apply(self, utc: NaiveDateTime, vm: &mut VM<'_>) -> RunResult<Value> {
        match self {
            Self::Nanoseconds => epoch_nanoseconds(utc).map(Value::Int),
            Self::Gmtime => Ok(gmtime_value(utc, vm)),
            Self::Localtime => localtime_value(utc, vm),
            Self::Ctime => {
                let fields = local_fields(utc, vm)?;
                Ok(allocate_string(fields.asctime(), vm.heap))
            }
            Self::Strftime(format) => {
                let fields = local_fields(utc, vm)?;
                let text = fields.strftime(&format, vm)?;
                Ok(allocate_string(text, vm.heap))
            }
        }
    }
}

/// Resume half of [`suspend_for_clock`]: turns the host's `time.time` answer
/// (epoch seconds) into the value the suspended function returns.
pub(crate) fn apply_clock_reading(reading: ClockReading, reply: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let seconds = match reply {
        Value::Float(seconds) => Ok(seconds),
        Value::Int(seconds) => Ok(seconds as f64),
        other => {
            let type_name = other.py_type_name(vm);
            other.drop_with(vm);
            Err(ExcType::type_error(format!(
                "'time.time' must be answered with a float, not {type_name}"
            )))
        }
    }?;
    reading.apply(instant_from_seconds(seconds)?, vm)
}

/// `time.process_time()` / `thread_time()` and their `_ns` forms.
///
/// These exclude sleeps, so they read the session's `process_time` policy rather
/// than its clock, and never suspend. Monty has no threads, so the thread clock
/// is the process clock.
fn process_time(vm: &mut VM<'_>, args: ArgValues, function: TimeFunctions, nanoseconds: bool) -> RunResult<CallResult> {
    args.check_zero_args(&format!("time.{function}"), vm.heap)?;
    let elapsed = match vm.env.os_policy.process_time {
        ProcessTime::Zero => Duration::ZERO,
        ProcessTime::Elapsed => vm.heap.tracker.elapsed(),
    };
    Ok(CallResult::Value(if nanoseconds {
        // Saturates rather than overflowing: a session cannot run for 292 years,
        // and a wrapped negative reading would break the clock's monotonicity.
        Value::Int(i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
    } else {
        Value::Float(elapsed.as_secs_f64())
    }))
}

/// Validates the delay in every mode, then applies [`host_sleep`].
/// [`PostConversionEffect::DiscardResult`] makes the call return `None`
/// regardless of the host's answer; `Zero` skips the wait.
fn sleep(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    // METH_O in CPython: keywords are refused wholesale, before arity.
    let seconds = args
        .reject_kwargs("time.sleep", vm.heap)?
        .get_one_arg("time.sleep", vm.heap)?;
    let result = sleep_seconds(&seconds, vm).and_then(|secs| {
        sleep_duration(secs).map_err(|err| match err {
            SleepError::NotANumber => ExcType::value_error("Invalid value NaN (not a number)"),
            SleepError::Negative => ExcType::value_error("sleep length must be non-negative"),
            SleepError::TooLarge => ExcType::sleep_too_long(),
        })
    });
    seconds.drop_with(vm.heap);
    let duration = result?;
    let call = match host_sleep(vm, duration) {
        Some(HostSleep::System(delay)) => OsFunctionCall::SystemSleep(delay),
        Some(HostSleep::CallHost(delay)) => OsFunctionCall::Sleep(delay),
        None => return Ok(CallResult::Value(Value::None)),
    };
    Ok(CallResult::OsCallWithEffect {
        call,
        effect: PostConversionEffect::DiscardResult.into(),
    })
}

/// Sleep destination and delay after applying the session policy.
pub(crate) enum HostSleep {
    /// The host itself, for a delay already cut to the mode's maximum.
    System(Duration),
    /// The host's `os` handler, with the requested delay uncapped.
    CallHost(Duration),
}

/// Applies the sleep policy, returning `None` for `SleepMode::Zero`.
/// The call kind tells the host who waits without needing the session policy.
pub(crate) fn host_sleep(vm: &VM<'_>, delay: Duration) -> Option<HostSleep> {
    match vm.env.os_policy.sleep {
        SleepMode::System(max) => Some(HostSleep::System(delay.min(max))),
        SleepMode::CallHost => Some(HostSleep::CallHost(delay)),
        SleepMode::Zero => None,
    }
}

/// The float seconds an already-`__index__`ed value denotes; an integer too
/// large for `f64` becomes infinity for the caller's range check to reject.
fn index_seconds(index: &Value, vm: &VM<'_>) -> f64 {
    match index {
        Value::Int(n) => *n as f64,
        Value::Bool(b) => f64::from(*b),
        other => other
            .as_long_int(vm)
            .map_or(0.0, |n| n.to_f64().unwrap_or(f64::INFINITY)),
    }
}

/// Converts a sleep length to float seconds the way CPython's
/// `_PyTime_FromSecondsObject` does: a float passes straight through, and
/// anything else goes through `__index__`, so a `str` — or a class with only
/// `__float__` — is rejected as non-integral rather than as a non-number.
///
/// An integer too large for `f64` becomes infinity, which the caller's range
/// check turns into the same `OverflowError` CPython raises for it.
fn sleep_seconds(value: &Value, vm: &mut VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        _ => match value.py_index_impl(vm)? {
            Some(index) => {
                let seconds = index_seconds(&index, vm);
                index.drop_with(vm);
                Ok(seconds)
            }
            None => Err(ExcType::type_error_not_integer_or_float(&value.py_type_name(vm))),
        },
    }
}

/// `time.mktime(t)`: the inverse of `localtime`, reading `t` as wall time in the
/// session zone. `METH_O` in CPython, hence the module-qualified arity message.
fn mktime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let value = args
        .reject_kwargs("time.mktime", vm.heap)?
        .get_one_arg("time.mktime", vm.heap)?;
    // Only the wall clock matters, and CPython does not range-check the
    // weekday or year day here as `asctime`/`strftime` do.
    let wall = time_tuple_parts(&value, "mktime", vm).and_then(|(items, _)| naive_from_time_tuple(&items));
    value.drop_with(vm.heap);
    let utc = vm
        .env
        .os_policy
        .timezone
        .utc_from_local(wall?)
        .ok_or_else(ExcType::mktime_out_of_range)?;
    Ok(CallResult::Value(Value::Float(unix_seconds(utc).floor())))
}

/// `time.asctime(t=None)`: the fixed 24-character form, from `t` or the clock.
/// `PyArg_UnpackTuple` in CPython, unlike its `gmtime` neighbours.
fn asctime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let Some(value) = args
        .reject_kwargs("asctime", vm.heap)?
        .get_zero_one_arg("asctime", vm.heap)?
    else {
        return suspend_or_apply(vm, TimeCaller::Asctime, ClockReading::Ctime);
    };
    let fields = time_fields(&value, "asctime", vm);
    value.drop_with(vm.heap);
    Ok(CallResult::Value(allocate_string(fields?.asctime(), vm.heap)))
}

/// `time.strftime(format, t=None)`, formatting `t` or the clock.
fn strftime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let StrftimeArgs { format, t } = StrftimeArgs::from_args(args, vm)?;
    let result = strftime_result(&format, &t, vm);
    (format, t).drop_with(vm.heap);
    result
}

/// `time.strftime(format, t=None)`; `PyArg_ParseTuple` in CPython.
#[derive(FromArgs)]
#[from_args(name = "strftime", style = parse_tuple)]
struct StrftimeArgs {
    #[from_args(pos_only)]
    format: Value,
    #[from_args(pos_only, default = Value::None)]
    t: Value,
}

/// [`strftime`] without the argument cleanup, so every exit releases both.
fn strftime_result(format: &Value, t: &Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let format = string_arg(format, "strftime", 1, vm)?;
    if matches!(t, Value::None) {
        suspend_or_apply(vm, TimeCaller::Strftime, ClockReading::Strftime(format.into()))
    } else {
        let text = time_fields(t, "strftime", vm)?.strftime(&format, vm)?;
        Ok(CallResult::Value(allocate_string(text, vm.heap)))
    }
}

/// `time.strptime(string, format='%a %b %d %H:%M:%S %Y')`.
///
/// CPython's is the pure-Python `_strptime_time`, which is why the binding
/// errors name that function rather than `strptime`.
fn strptime(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let StrptimeArgs { data_string, format } = StrptimeArgs::from_args(args, vm)?;
    let result = strptime_result(&data_string, &format, vm);
    (data_string, format).drop_with(vm.heap);
    result.map(CallResult::Value)
}

/// `time.strptime(data_string, format)` — the pure-Python `_strptime_time`.
#[derive(FromArgs)]
#[from_args(name = "_strptime_time", style = def)]
struct StrptimeArgs {
    data_string: Value,
    #[from_args(default = Value::None)]
    format: Value,
}

/// [`strptime`] without the argument cleanup.
fn strptime_result(data_string: &Value, format: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let text = string_arg(data_string, "strptime", 1, vm)?;
    let format = match format {
        Value::None => DEFAULT_STRPTIME_FORMAT.to_owned(),
        other => string_arg(other, "strptime", 2, vm)?,
    };
    let wall = datetime_type::parse_time_strptime(&text, &format)?;
    // A `%z` offset is parsed but dropped, so the zone half stays unset; CPython
    // records it in `tm_gmtoff` (see `limitations/time.md`).
    Ok(struct_time_value(&TimeFields::naive(wall), vm))
}

/// `time.strptime`'s format when the caller gives none: `asctime`'s form.
const DEFAULT_STRPTIME_FORMAT: &str = "%a %b %d %H:%M:%S %Y";

/// Reads the clock for a conversion that had no explicit time, suspending to the
/// host when it owns the clock.
fn suspend_or_apply(vm: &mut VM<'_>, caller: TimeCaller, reading: ClockReading) -> RunResult<CallResult> {
    match sandbox_instant(vm)? {
        Some(utc) => reading.apply(utc, vm).map(CallResult::Value),
        None => Ok(suspend_for_clock(caller, Some(reading))),
    }
}

/// The `str` at position `position` of a `time` function's arguments,
/// with `_PyArg_BadArgument`'s positional wording.
fn string_arg(value: &Value, name: &str, position: usize, vm: &VM<'_>) -> RunResult<String> {
    match value.to_str(vm) {
        Ok(text) => Ok(text.to_owned()),
        Err(_) => Err(ExcType::type_error(format!(
            "{name}() argument {position} must be str, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// A broken-down time, the payload every `struct_time` carries.
///
/// `weekday` and `yearday` are stored rather than derived because CPython
/// formats them as given: `strftime('%a', (…, tm_wday=0, …))` prints `Mon`
/// even when the date is a Saturday.
struct TimeFields {
    wall: NaiveDateTime,
    weekday: i64,
    yearday: i64,
    /// -1 when unknown (`strptime`), else 0 or 1.
    isdst: i64,
    /// The zone the wall clock is expressed in; `None` leaves `tm_gmtoff` and
    /// `tm_zone` unset, as CPython does for a zone-less `strptime`.
    zone: Option<MontyTimeZone>,
}

impl TimeFields {
    /// Fields for a wall clock in `zone`, deriving the weekday and year day.
    fn zoned(wall: NaiveDateTime, zone: MontyTimeZone, isdst: i64) -> Self {
        Self {
            wall,
            weekday: i64::from(wall.weekday().num_days_from_monday()),
            yearday: i64::from(wall.ordinal()),
            isdst,
            zone: Some(zone),
        }
    }

    /// `tm_wday` folded into `0..7`, Monday first, as an index into the day
    /// names. The tuple can carry any integer here; CPython folds it the same way.
    fn weekday_index(&self) -> usize {
        usize::try_from(self.weekday.rem_euclid(7)).unwrap_or_default()
    }

    /// Fields with no zone, for `strptime` and a bare 9-element tuple.
    fn naive(wall: NaiveDateTime) -> Self {
        Self {
            wall,
            weekday: i64::from(wall.weekday().num_days_from_monday()),
            yearday: i64::from(wall.ordinal()),
            isdst: -1,
            zone: None,
        }
    }

    /// CPython's fixed 24-character `asctime` form, built from the stored
    /// weekday rather than the date's own.
    fn asctime(&self) -> String {
        const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let day = DAYS[self.weekday_index()];
        let month = MONTHS[(self.wall.month0() as usize).min(11)];
        format!(
            "{day} {month} {:2} {:02}:{:02}:{:02} {}",
            self.wall.day(),
            self.wall.hour(),
            self.wall.minute(),
            self.wall.second(),
            self.wall.year()
        )
    }

    /// Applies a `strftime` format, with the zone directives from the stored zone
    /// (as `datetime.strftime` does) and the day directives from the stored fields.
    /// `%f` renders zeros via the shared rewrite; CPython's `time.strftime` has no
    /// microsecond field at all (see `limitations/time.md`).
    fn strftime(&self, format: &str, vm: &mut VM<'_>) -> RunResult<String> {
        let (offset, name) = match &self.zone {
            Some(zone) => (Some(zone.offset_seconds), zone.name.clone()),
            None => (None, None),
        };
        let format = self.rewrite_day_directives(format, &vm.heap.tracker)?;
        let format = date::rewrite_zone_directives(&format, offset, name.as_deref(), &vm.heap.tracker)?;
        let format = date::rewrite_microsecond_directive(&format);
        let rendered = self.wall.format_with_items(StrftimeItems::new_lenient(&format));
        date::render_strftime(rendered).ok_or_else(date::invalid_strftime_error)
    }

    /// Substitutes the directives CPython reads from `tm_wday`/`tm_yday`, which
    /// chrono would recompute from the date. Shifting the date instead is not an
    /// option: `%d`, `%m` and `%Y` must still come from the date itself.
    fn rewrite_day_directives<'f>(&self, format: &'f str, tracker: &ResourceTracker) -> RunResult<Cow<'f, str>> {
        const DAYS: [(&str, &str); 7] = [
            ("Mon", "Monday"),
            ("Tue", "Tuesday"),
            ("Wed", "Wednesday"),
            ("Thu", "Thursday"),
            ("Fri", "Friday"),
            ("Sat", "Saturday"),
            ("Sun", "Sunday"),
        ];
        let may_have_day = format
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'%' && matches!(pair[1], b'a' | b'A' | b'j' | b'w' | b'u' | b'U' | b'W'));
        if !may_have_day {
            return Ok(Cow::Borrowed(format));
        }
        // C counts weekdays from Sunday and year days from zero, which is what
        // the `%w`, `%U` and `%W` formulas below are written against.
        let monday_based = self.weekday_index();
        let (short, long) = DAYS[monday_based];
        let sunday_based = (monday_based + 1) % 7;
        let c_yearday = self.yearday - 1;
        let sunday_based = i64::try_from(sunday_based).unwrap_or_default();
        let week = |first_weekday: i64| (c_yearday + 7 - (sunday_based + 7 - first_weekday) % 7) / 7;
        let mut out = StringBuilder::with_capacity(format.len(), tracker)?;
        let mut rest = format;
        while let Some(percent) = rest.find('%') {
            let (before, from_percent) = rest.split_at(percent);
            out.push_str(before)?;
            let directive = from_percent[1..].chars().next();
            let substitute = match directive {
                Some('a') => Some(short.to_owned()),
                Some('A') => Some(long.to_owned()),
                Some('j') => Some(format!("{:03}", self.yearday)),
                Some('w') => Some(sunday_based.to_string()),
                Some('u') => Some((monday_based + 1).to_string()),
                Some('U') => Some(format!("{:02}", week(0))),
                Some('W') => Some(format!("{:02}", week(1))),
                _ => None,
            };
            let consumed = match (substitute, directive) {
                (Some(text), Some(c)) => {
                    out.push_str(&text)?;
                    1 + c.len_utf8()
                }
                // Copy the directive whole, so `%%a` consumes `%%` and leaves `a` as text.
                (None, Some(c)) => {
                    out.push('%')?;
                    out.push(c)?;
                    1 + c.len_utf8()
                }
                (_, None) => {
                    out.push('%')?;
                    1
                }
            };
            rest = &from_percent[consumed..];
        }
        out.push_str(rest)?;
        out.finish_raw().map(Cow::Owned)
    }
}

/// `time.gmtime(secs)`: the instant broken down in UTC.
fn gmtime_value(utc: NaiveDateTime, vm: &mut VM<'_>) -> Value {
    let zone = MontyTimeZone {
        offset_seconds: 0,
        name: Some("UTC".to_owned()),
    };
    struct_time_value(&TimeFields::zoned(utc, zone, 0), vm)
}

/// `time.localtime(secs)`: the instant broken down in the session zone.
fn localtime_value(utc: NaiveDateTime, vm: &mut VM<'_>) -> RunResult<Value> {
    let fields = local_fields(utc, vm)?;
    Ok(struct_time_value(&fields, vm))
}

/// The session zone's reading of `utc`, with `tm_isdst` set from whether the
/// zone is on its daylight half at that instant.
fn local_fields(utc: NaiveDateTime, vm: &VM<'_>) -> RunResult<TimeFields> {
    let zone = vm.env.os_policy.timezone.at(utc);
    let isdst = i64::from(is_daylight(&vm.env.os_policy.timezone, utc, &zone));
    let wall = sandbox_local_wall_clock(vm, utc)?;
    Ok(TimeFields::zoned(wall, zone, isdst))
}

/// Whether `zone` is the daylight half of the session zone at `utc`, which is
/// `tm_isdst`. A fixed zone has one half, so it is never daylight.
fn is_daylight(sandbox_zone: &SandboxTimeZone, utc: NaiveDateTime, at: &MontyTimeZone) -> bool {
    sandbox_zone
        .constants(Some(utc.year()))
        .is_some_and(|constants| constants.daylight && at.offset_seconds == constants.daylight_zone.offset_seconds)
}

/// Builds a `time.struct_time`.
///
/// A structseq in CPython: no class object, so `_fields`/`_replace` are absent
/// here too. Unlike CPython's, the two zone fields are part of the tuple, so
/// `len()` is 11 (see `limitations/time.md`).
fn struct_time_value(fields: &TimeFields, vm: &mut VM<'_>) -> Value {
    const NAMES: [StaticStrings; 11] = [
        StaticStrings::TmYear,
        StaticStrings::TmMon,
        StaticStrings::TmMday,
        StaticStrings::TmHour,
        StaticStrings::TmMin,
        StaticStrings::TmSec,
        StaticStrings::TmWday,
        StaticStrings::TmYday,
        StaticStrings::TmIsdst,
        StaticStrings::TmGmtoff,
        StaticStrings::TmZone,
    ];
    let wall = fields.wall;
    let (gmtoff, zone_name) = match &fields.zone {
        Some(zone) => (
            Value::Int(i64::from(zone.offset_seconds)),
            allocate_string(tzname_string(zone.offset_seconds, zone.name.as_deref()), vm.heap),
        ),
        None => (Value::None, Value::None),
    };
    let named_tuple = NamedTuple::new(
        EitherStr::Interned(vm.interns.intern_static(StaticStrings::StructTime)),
        NAMES
            .into_iter()
            .map(|name| EitherStr::Interned(vm.interns.intern_static(name)))
            .collect(),
        vec![
            Value::Int(i64::from(wall.year())),
            Value::Int(i64::from(wall.month())),
            Value::Int(i64::from(wall.day())),
            Value::Int(i64::from(wall.hour())),
            Value::Int(i64::from(wall.minute())),
            Value::Int(i64::from(wall.second())),
            Value::Int(fields.weekday),
            Value::Int(fields.yearday),
            Value::Int(fields.isdst),
            gmtoff,
            zone_name,
        ],
    );
    Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(named_tuple))))
}

/// Reads the `struct_time` (or bare 9-element tuple) that `mktime`, `asctime`
/// and `strftime` accept.
///
/// CPython requires exactly 9 items; Monty's own `struct_time` carries 11, so
/// both lengths are taken. A bare tuple has no zone of its own, so `tm_isdst`
/// selects the session zone's standard or daylight half, as CPython does.
fn time_fields(value: &Value, name: &str, vm: &mut VM<'_>) -> RunResult<TimeFields> {
    let (items, own_zone) = time_tuple_parts(value, name, vm)?;
    let wall = naive_from_time_tuple(&items)?;
    // CPython's `checktm`, which `mktime` skips: a weekday below -1 goes
    // negative after its Sunday-first shift, and a year day is 0..=366 with 0
    // read as the first day. Bounding both here also keeps the `%U`/`%W`
    // arithmetic in range, so a hostile tuple cannot overflow it.
    if items[6] < -1 {
        return Err(ExcType::value_error("day of week out of range"));
    }
    if !(0..=366).contains(&items[7]) {
        return Err(ExcType::value_error("day of year out of range"));
    }
    let isdst = items[8].clamp(-1, 1);
    let zone = own_zone.or_else(|| tuple_zone(wall.year(), isdst, vm));
    Ok(TimeFields {
        wall,
        weekday: items[6],
        yearday: items[7].max(1),
        isdst,
        zone,
    })
}

/// The nine core integers of a time tuple, plus the zone an 11-field
/// `struct_time` carries. Rejects anything else with CPython's wording.
fn time_tuple_parts(value: &Value, name: &str, vm: &VM<'_>) -> RunResult<([i64; 9], Option<MontyTimeZone>)> {
    let items = match value.read_heap(vm) {
        Some(HeapReadOutput::Tuple(tuple)) => tuple.get(vm.heap).as_slice(),
        Some(HeapReadOutput::NamedTuple(tuple)) => tuple.get(vm.heap).items(),
        _ => return Err(ExcType::type_error("Tuple or struct_time argument required".to_owned())),
    };
    if !matches!(items.len(), 9 | 11) {
        return Err(ExcType::illegal_time_tuple(name));
    }
    let mut core = [0_i64; 9];
    for (slot, item) in core.iter_mut().zip(items) {
        *slot = match item {
            Value::Int(n) => *n,
            Value::Bool(b) => i64::from(*b),
            other => {
                return Err(ExcType::type_error(format!(
                    "'{}' object cannot be interpreted as an integer",
                    other.py_type_name(vm)
                )));
            }
        };
    }
    // Only Monty's own `struct_time` has the two zone fields; a zone-less one
    // (from `strptime`) leaves them `None` and falls back to the session zone.
    let zone = match items.get(9..11) {
        Some([Value::Int(offset_seconds), name]) => {
            i32::try_from(*offset_seconds).ok().map(|offset_seconds| MontyTimeZone {
                offset_seconds,
                name: name.to_str(vm).ok().map(str::to_owned),
            })
        }
        _ => None,
    };
    Ok((core, zone))
}

/// The wall clock a time tuple's first six fields denote, with CPython's
/// per-field `ValueError` for one out of range.
fn naive_from_time_tuple(items: &[i64; 9]) -> RunResult<NaiveDateTime> {
    const RANGES: [(&str, i64, i64); 5] = [
        ("month", 1, 12),
        ("day of month", 1, 31),
        ("hour", 0, 23),
        ("minute", 0, 59),
        ("seconds", 0, 61),
    ];
    let mut checked = [0_u32; 5];
    for ((field, low, high), (slot, value)) in RANGES.into_iter().zip(checked.iter_mut().zip(&items[1..6])) {
        *slot = u32::try_from(*value)
            .ok()
            .filter(|_| (low..=high).contains(value))
            .ok_or_else(|| ExcType::value_error(format!("{field} out of range")))?;
    }
    let [month, day, hour, minute, second] = checked;
    let year = i32::try_from(items[0]).map_err(|_| ExcType::value_error("year out of range"))?;
    // CPython accepts leap seconds here and folds them into the next minute.
    let (second, extra) = if second > 59 {
        (59, i64::from(second) - 59)
    } else {
        (second, 0)
    };
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, second))
        .and_then(|wall| wall.checked_add_signed(TimeDelta::seconds(extra)))
        .ok_or_else(|| ExcType::value_error("day of month out of range"))
}

/// The zone a bare 9-element tuple denotes: the session zone's standard half,
/// or its daylight half when `tm_isdst` is 1.
fn tuple_zone(year: i32, isdst: i64, vm: &VM<'_>) -> Option<MontyTimeZone> {
    let constants = vm.env.os_policy.timezone.constants(Some(year))?;
    Some(if isdst == 1 {
        constants.daylight_zone
    } else {
        constants.standard
    })
}

/// The instant an optional `secs` argument denotes; `None` means the caller
/// passed nothing and the clock must be read instead.
fn optional_instant(seconds: &Value, vm: &mut VM<'_>) -> RunResult<Option<NaiveDateTime>> {
    match seconds {
        Value::None => sandbox_instant(vm),
        value => {
            let seconds = clock_seconds(value, vm)?;
            instant_from_seconds(seconds).map(Some)
        }
    }
}

/// The `secs` argument of `gmtime`/`localtime`/`ctime` as float seconds.
///
/// Like [`sleep_seconds`] a float passes through and anything else goes through
/// `__index__`, but CPython's `_PyTime_ObjectToTime_t` reports a rejection as a
/// plain non-integer rather than naming floats as acceptable.
fn clock_seconds(value: &Value, vm: &mut VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        _ => match value.py_index_impl(vm)? {
            Some(index) => {
                let seconds = index_seconds(&index, vm);
                index.drop_with(vm);
                Ok(seconds)
            }
            None => Err(ExcType::type_error(format!(
                "'{}' object cannot be interpreted as an integer",
                value.py_type_name(vm)
            ))),
        },
    }
}

/// Epoch seconds as an instant. The whole seconds floor, as CPython's
/// `_PyTime_ObjectToTime_t` does for `gmtime(-1.9)`, and the fraction is kept
/// so the `_ns` clocks see it; the broken-down fields never read it.
fn instant_from_seconds(seconds: f64) -> RunResult<NaiveDateTime> {
    let floored = seconds.floor();
    if floored.is_nan() {
        return Err(ExcType::value_error("Invalid value NaN (not a number)"));
    }
    // `as` saturates, which would silently clamp rather than raise.
    if !(-9.3e18..=9.3e18).contains(&floored) {
        return Err(ExcType::timestamp_out_of_range());
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "range-checked above, and already floored"
    )]
    let whole = floored as i64;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a fraction in 0..1, scaled"
    )]
    let nanos = (((seconds - floored) * 1e9).round() as u32).min(999_999_999);
    DateTime::from_timestamp(whole, nanos)
        .map(|utc| utc.naive_utc())
        .filter(|utc| local_wall_clock(*utc, 0).is_some())
        .ok_or_else(ExcType::timestamp_out_of_range)
}

/// Nanoseconds since the epoch, for the `_ns` clocks. Chrono's range is about
/// 1677..2262, narrower than `datetime`'s, so a fixed clock outside it raises.
fn epoch_nanoseconds(utc: NaiveDateTime) -> RunResult<i64> {
    utc.and_utc()
        .timestamp_nanos_opt()
        .ok_or_else(ExcType::timestamp_out_of_range)
}
