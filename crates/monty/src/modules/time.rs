//! Implementation of the `time` module.
//!
//! `time()` and `sleep()` follow the session's `AutoOsCalls` policies, and the
//! zone constants (`timezone`, `altzone`, `daylight`, `tzname`) describe its
//! zone. Monotonic clocks and the `struct_time` family raise `AttributeError`.
//! See `limitations/time.md` for CPython divergences.

use std::time::Duration;

use monty_types::{OsFunctionCall, SandboxTimeZone, SleepError, SleepMode, sleep_duration, unix_seconds};
use num_traits::ToPrimitive;
use smallvec::smallvec;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::PostConversionEffect,
    types::{
        Module, PyTrait, datetime::sandbox_instant, str::allocate_string, timezone::tzname_string,
        tuple::allocate_tuple,
    },
    value::Value,
};

/// `time` module functions, each a Python-visible callable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum TimeFunctions {
    Time,
    Sleep,
}

/// Creates the `time` module and allocates it on the heap.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Time, vm.interns);

    module.set_attr(
        StaticStrings::Time,
        Value::ModuleFunction(ModuleFunctions::Time(TimeFunctions::Time)),
        vm,
    );
    module.set_attr(
        StaticStrings::Sleep,
        Value::ModuleFunction(ModuleFunctions::Time(TimeFunctions::Sleep)),
        vm,
    );
    set_zone_constants(&mut module, vm);

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Sets `timezone`, `altzone`, `daylight` and `tzname` from the sandbox zone,
/// the values CPython reads from libc. A fixed zone has no DST, so both offsets
/// and both names agree. `CallHost` leaves them unset: module creation cannot
/// suspend for the host's answer.
fn set_zone_constants(module: &mut Module, vm: &mut VM<'_>) {
    let SandboxTimeZone::Fixed { offset_seconds, name } = vm.env.auto_os_calls.timezone.clone() else {
        return;
    };
    // `time.timezone` is seconds *west* of UTC, the opposite sign to `utcoffset()`.
    let west = Value::Int(-i64::from(offset_seconds));
    module.set_attr(StaticStrings::Timezone, west.clone_with_heap(vm.heap), vm);
    module.set_attr(StaticStrings::Altzone, west, vm);
    module.set_attr(StaticStrings::Daylight, Value::Int(0), vm);
    let name = allocate_string(tzname_string(offset_seconds, name.as_deref()), vm.heap);
    let tzname = allocate_tuple(smallvec![name.clone_with_heap(vm.heap), name], vm.heap);
    module.set_attr(StaticStrings::Tzname, tzname, vm);
}

/// Dispatches a call to a `time` module function.
pub(super) fn call(vm: &mut VM<'_>, function: TimeFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        TimeFunctions::Time => time(vm, args),
        TimeFunctions::Sleep => sleep(vm, args),
    }
}

/// Reads epoch seconds from the session's clock, or the host under `CallHost`.
fn time(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    args.check_zero_args("time.time", vm.heap)?;
    match sandbox_instant(vm)? {
        None => Ok(CallResult::OsCall(OsFunctionCall::Time)),
        Some(utc) => Ok(CallResult::Value(Value::Float(unix_seconds(utc)))),
    }
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
    match vm.env.auto_os_calls.sleep {
        SleepMode::System(max) => Some(HostSleep::System(delay.min(max))),
        SleepMode::CallHost => Some(HostSleep::CallHost(delay)),
        SleepMode::Zero => None,
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
                let seconds = match index {
                    Value::Int(n) => n as f64,
                    Value::Bool(b) => f64::from(b),
                    ref other => other
                        .as_long_int(vm)
                        .map_or(0.0, |n| n.to_f64().unwrap_or(f64::INFINITY)),
                };
                index.drop_with(vm);
                Ok(seconds)
            }
            None => Err(ExcType::type_error_not_integer_or_float(&value.py_type_name(vm))),
        },
    }
}
