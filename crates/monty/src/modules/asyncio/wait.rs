//! `asyncio.wait` state construction and timeout handling.
//!
//! The heap-side state machine lives in [`WaitFuture`](crate::asyncio::WaitFuture);
//! this module validates the call, starts an optional host timer, and returns
//! the awaitable.

use monty_types::{OsFunctionCall, sleep_duration_saturating};
use num_traits::ToPrimitive;

use crate::{
    args::{ArgValues, FromArgs},
    asyncio::{UnawaitedWait, WaitFuture, WaitFutureState},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::HeapData,
    heap_traits::DropGuard,
    modules::time::{HostSleep, host_sleep},
    os_dispatch::{PostConversionEffect, SleepDestination},
    value::Value,
};

/// Implementation of `asyncio.wait(fs, *, timeout=None, return_when=ALL_COMPLETED)`.
///
/// CPython implements this as a coroutine, so validation and iteration happen
/// at first await; [`WaitFuture`] keeps that shape. Returns `(done, pending)`
/// sets. See `limitations/asyncio.md`.
pub(crate) fn wait(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let WaitArgs {
        fs,
        timeout,
        return_when,
    } = WaitArgs::from_args(args, vm)?;

    let mut args_guard = DropGuard::new(((fs, return_when), timeout.unwrap_or(Value::None)), vm);

    let (timed_out, call) = {
        let ((_, timeout), vm) = args_guard.as_parts_mut();
        resolve_wait_timeout(timeout, vm)?
    };

    let (((fs, return_when), timeout), vm) = args_guard.into_parts();
    timeout.drop_with(vm);

    let wait_id = vm.heap.allocate(HeapData::WaitFuture(Box::new(WaitFuture {
        state: WaitFutureState::Unawaited(UnawaitedWait {
            fs,
            return_when,
            timer: None,
            timed_out,
        }),
    })));

    let wait = Value::Ref(wait_id);

    match call {
        Some(call) => Ok(CallResult::OsCallWithEffect {
            call,
            effect: PostConversionEffect::SleepResult {
                result: Value::None,
                destination: SleepDestination::Wait(wait),
            }
            .into(),
        }),
        None => Ok(CallResult::Value(wait)),
    }
}

/// Resolves the timeout into whether it has already expired and the optional
/// host sleep call that arms the remaining timer.
fn resolve_wait_timeout(timeout: &Value, vm: &mut VM<'_>) -> RunResult<(bool, Option<OsFunctionCall>)> {
    if matches!(timeout, Value::None) {
        return Ok((false, None));
    }

    let seconds = timeout_seconds(timeout, vm)?;
    if seconds.is_nan() || seconds <= 0.0 {
        return Ok((true, None));
    }

    let delay =
        sleep_duration_saturating(seconds).map_err(|_| ExcType::value_error("Invalid delay: NaN (not a number)"))?;

    Ok(match host_sleep(vm, delay) {
        Some(HostSleep::CallHost(delay)) => (false, Some(OsFunctionCall::AsyncSleep(delay))),
        Some(HostSleep::System(delay)) if !delay.is_zero() => (false, Some(OsFunctionCall::AsyncSystemSleep(delay))),
        _ => (true, None),
    })
}

/// `asyncio.wait(fs, *, timeout=None, return_when=...)` — a pure-Python `def`,
/// so no argument is type-checked while binding.
#[derive(FromArgs)]
#[from_args(name = "wait", style = def)]
struct WaitArgs {
    fs: Value,
    #[from_args(kw_only, default = None)]
    timeout: Option<Value>,
    #[from_args(kw_only, default = None)]
    return_when: Option<Value>,
}

/// Reads a wait timeout as float seconds.
fn timeout_seconds(value: &Value, vm: &VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        Value::Int(n) => Ok(*n as f64),
        Value::Bool(b) => Ok(f64::from(*b)),
        _ => match value.as_long_int(vm) {
            Some(n) => Ok(n.to_f64().unwrap_or(f64::INFINITY)),
            None => Err(SimpleException::new_msg(
                ExcType::TypeError,
                format!(
                    "unsupported operand type(s) for +: 'float' and '{}'",
                    value.py_type_name(vm)
                ),
            )
            .into()),
        },
    }
}
