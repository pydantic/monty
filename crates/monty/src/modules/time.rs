//! Implementation of the `time` module.
//!
//! Two functions, both of which need the host: `time()` reads its clock and
//! `sleep()` asks it to wait. Neither is served in the interpreter — they
//! yield an [`OsFunctionCall`] the host permits, serves or refuses, exactly
//! like `date.today()`. See `limitations/time.md` for what diverges from
//! CPython; the monotonic clocks and the `struct_time` family are absent
//! rather than stubbed, so they raise `AttributeError` up front.

use monty_types::{OsFunctionCall, SleepError, sleep_duration};
use num_traits::ToPrimitive;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::PostConversionEffect,
    types::{Module, PyTrait},
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

/// `time.time()` — seconds since the Unix epoch, as a float.
///
/// The host answers from whatever clock it exposes, so the value need not
/// agree with the machine's wall clock (and a host with no clock refuses it).
fn time(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    args.check_zero_args("time.time", vm.heap)?;
    Ok(CallResult::OsCall(OsFunctionCall::Time))
}

/// `time.sleep(seconds)` — suspend until the host says the wait is over.
///
/// The sandbox holds no clock and cannot block, so the wait is the host's to
/// perform; [`PostConversionEffect::DiscardResult`] then makes the call
/// evaluate to `None` whatever the host answered with, matching CPython.
///
/// `max_duration` does not run while the sandbox is suspended, so a sleep is
/// bounded by the host's own turn deadline and by `max_suspensions` rather
/// than by the execution-time limit (see `limitations/time.md`).
fn sleep(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    // METH_O in CPython: keywords are refused wholesale, before arity.
    let seconds = args
        .reject_kwargs("time.sleep", vm.heap)?
        .get_one_arg("time.sleep", vm.heap)?;
    let from_float = matches!(seconds, Value::Float(_));
    let result = sleep_seconds(&seconds, vm).and_then(|secs| {
        sleep_duration(secs).map_err(|err| match err {
            SleepError::NotANumber => ExcType::value_error("Invalid value NaN (not a number)"),
            SleepError::Negative => ExcType::value_error("sleep length must be non-negative"),
            SleepError::TooLarge => ExcType::sleep_too_long(from_float),
        })
    });
    seconds.drop_with(vm.heap);
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::Sleep(result?),
        effect: PostConversionEffect::DiscardResult.into(),
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
            None => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
        },
    }
}
