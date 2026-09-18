//! Implementation of the `time` module.
//!
//! Two functions, `time()` and `sleep()`, each answered in the sandbox or
//! suspended to the host as the session's `AutoOsCalls` says, exactly like
//! `date.today()`. See `limitations/time.md` for what diverges from CPython;
//! the monotonic clocks and the `struct_time` family are absent rather than
//! stubbed, so they raise `AttributeError` up front.

use monty_types::{OsFunctionCall, SleepError, SleepMode, sleep_duration};
use num_traits::ToPrimitive;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::PostConversionEffect,
    types::{Module, PyTrait, datetime::sandbox_now},
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

    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to a `time` module function.
pub(super) fn call(vm: &mut VM<'_>, function: TimeFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        TimeFunctions::Time => time(vm, args),
        TimeFunctions::Sleep => sleep(vm, args),
    }
}

/// `time.time()` — seconds since the Unix epoch, as a float, read from the
/// session's clock. Under `CallHost` the host answers from whatever clock it
/// exposes, so the value need not agree with the machine's wall clock.
fn time(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    args.check_zero_args("time.time", vm.heap)?;
    match sandbox_now(vm)? {
        None => Ok(CallResult::OsCall(OsFunctionCall::Time)),
        Some(reading) => Ok(CallResult::Value(Value::Float(reading.unix_seconds()))),
    }
}

/// `time.sleep(seconds)` — wait as the session's `SleepMode` says.
///
/// A sandbox wait is cut to `sandbox_sleep_clamp` and runs off the execution
/// clock, so it counts against neither `max_feed_duration` nor `max_suspensions`.
/// Under `CallHost` the wait is the host's to perform, and
/// [`PostConversionEffect::DiscardResult`] makes the call evaluate to `None`
/// whatever the host answered with. The argument is validated identically in
/// every mode, so the CPython errors do not depend on the mode.
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
    let calls = vm.env.auto_os_calls;
    Ok(match calls.sleep {
        SleepMode::CallHost => CallResult::OsCallWithEffect {
            call: OsFunctionCall::Sleep(duration),
            effect: PostConversionEffect::DiscardResult.into(),
        },
        SleepMode::Zero => CallResult::Value(Value::None),
        SleepMode::SandboxSleep => {
            vm.heap.tracker.sandbox_sleep(duration.min(calls.sandbox_sleep_clamp));
            CallResult::Value(Value::None)
        }
    })
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
