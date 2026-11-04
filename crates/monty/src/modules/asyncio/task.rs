//! Python-visible `asyncio.Task` construction and methods.
//!
//! [`create_task`] wraps a coroutine in a scheduler task or adopts a host
//! `ExternalFuture`; [`task_call_method`] serves the non-suspending `Task`
//! attributes. The rest of the `asyncio` surface lives in the parent module.

use crate::{
    args::{ArgValues, FromArgs},
    asyncio::{Awaiter, ExternalFutureState, TaskFuture, TaskFutureState},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{HeapData, HeapId, HeapObjectRead, HeapReadOutput},
    heap_traits::{DropGuard, DropWithContext},
    intern::StaticStrings,
    types::{PyTrait, Type, str::allocate_string},
    value::{EitherStr, Value},
};

/// Implementation of `asyncio.create_task(coro, *, name=None)`.
///
/// The wrapped coroutine is handed to the scheduler as a user task. An
/// external host future needs no scheduler task: it already owns its own
/// completion state, so this registers a [`Awaiter::UserTask`] that completes
/// the Python-visible task when it settles. A coroutine that is already
/// driving another task yields a task that fails on first access with
/// CPython's reuse error.
pub(crate) fn create_task(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CreateTaskArgs { coro, name } = CreateTaskArgs::from_args(args, vm)?;
    let mut coro_guard = DropGuard::new(coro, vm);

    let start = {
        let (coro, vm) = coro_guard.as_parts();

        match coro {
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Coroutine(_)) => TaskStart::Coroutine,
            Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::ExternalFuture(_)) => TaskStart::ExternalFuture,
            _ => {
                let rendered = coro.py_repr(vm)?;
                defer_drop!(rendered, vm);

                let text = value_to_string(rendered, vm)?;
                return Err(ExcType::type_error(format!("a coroutine was expected, got {text}")));
            }
        }
    };

    // CPython's running task is Task-1, so the first created task is Task-2.
    // Only a default name consumes a counter value, matching CPython.
    let name = {
        let vm = coro_guard.ctx();
        match name {
            Some(Value::None) | None => vm.allocate_task_name(),
            Some(value) => {
                defer_drop!(value, vm);
                value_to_string(value, vm)?
            }
        }
    };

    let (coro, vm) = coro_guard.into_parts();
    let coro_id = coro
        .into_ref_id()
        .expect("validated create_task argument is heap-backed");

    let handle = vm
        .heap
        .allocate(HeapData::Task(Box::new(TaskFuture::new(None, coro_id, name))));

    match start {
        TaskStart::Coroutine => start_coroutine_task(vm, coro_id, handle),
        TaskStart::ExternalFuture => start_external_future_task(vm, coro_id, handle),
    }

    Ok(Value::Ref(handle))
}

/// How `create_task` starts the supplied awaitable.
enum TaskStart {
    Coroutine,
    ExternalFuture,
}

/// Serves `asyncio.Task` attributes that are plain Python values.
///
/// Methods that can run bytecode are not here: `Task` has none in the
/// supported subset. `result` and `exception` clone the cached record, while
/// pending access raises CPython's `InvalidStateError`.
pub(crate) fn task_call_method<'h>(
    task: &HeapObjectRead<'h, TaskFuture>,
    attr: &EitherStr,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<CallResult> {
    let Some(name) = attr.static_string(vm.interns) else {
        args.drop_with(vm);

        return Err(ExcType::attribute_error(
            Type::Task.name(vm.heap, vm.interns),
            attr.as_str(vm.interns),
        ));
    };

    let value = match name {
        StaticStrings::Done => {
            args.check_zero_args("Task.done", vm.heap)?;
            Value::Bool(task.get(vm.heap).is_done())
        }
        StaticStrings::Cancelled => {
            args.check_zero_args("Task.cancelled", vm.heap)?;
            Value::Bool(false)
        }
        StaticStrings::Cancel => {
            let message = args.get_zero_one_arg("Task.cancel", vm.heap)?;
            message.drop_with(vm);

            // Cancellation is not part of the supported asyncio subset.
            Value::Bool(false)
        }
        StaticStrings::GetName => {
            args.check_zero_args("Task.get_name", vm.heap)?;
            allocate_string(task.get(vm.heap).name.clone(), vm.heap)
        }
        StaticStrings::SetName => {
            let name = args.get_one_arg("Task.set_name", vm.heap)?;
            defer_drop!(name, vm);

            let name = value_to_string(name, vm)?;
            task.get_mut(vm.heap).name = name;

            Value::None
        }
        StaticStrings::GetCoro => {
            args.check_zero_args("Task.get_coro", vm.heap)?;
            let id = task.get(vm.heap).coroutine_id;
            vm.heap.inc_ref(id);

            Value::Ref(id)
        }
        StaticStrings::Result => {
            args.check_zero_args("Task.result", vm.heap)?;
            match &task.get(vm.heap).state {
                TaskFutureState::Pending => {
                    return Err(SimpleException::new_msg(ExcType::InvalidStateError, "Result is not set.").into());
                }
                TaskFutureState::Completed(value) => value.clone_with_heap(vm),
                TaskFutureState::Failed(error) => return Err(error.clone()),
            }
        }
        StaticStrings::Exception => {
            args.check_zero_args("Task.exception", vm.heap)?;
            match &task.get(vm.heap).state {
                TaskFutureState::Pending => {
                    return Err(SimpleException::new_msg(ExcType::InvalidStateError, "Exception is not set.").into());
                }
                TaskFutureState::Completed(_) => Value::None,
                TaskFutureState::Failed(error) => {
                    let error = error.clone();
                    exception_value(&error, vm)
                }
            }
        }
        _ => {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(
                Type::Task.name(vm.heap, vm.interns),
                attr.as_str(vm.interns),
            ));
        }
    };
    Ok(CallResult::Value(value))
}

/// Queues a coroutine-backed task, or marks its handle failed when the
/// coroutine is already driving another scheduler task.
fn start_coroutine_task(vm: &mut VM<'_>, coro_id: HeapId, handle: HeapId) {
    if let Some(task_id) = vm.spawn_user_task(coro_id, handle) {
        let HeapReadOutput::Task(task) = vm.heap.read(handle) else {
            unreachable!("create_task allocated a non-task handle")
        };

        task.get_mut(vm.heap).task_id = Some(task_id);
    } else {
        let HeapReadOutput::Task(task) = vm.heap.read(handle) else {
            unreachable!("create_task allocated a non-task handle")
        };

        task.get_mut(vm.heap).state = TaskFutureState::Failed(ExcType::cannot_reuse_already_awaited_coroutine());
    }
}

/// Adopts a host future as the state source for a Python-visible task.
fn start_external_future_task(vm: &mut VM<'_>, future_id: HeapId, handle: HeapId) {
    let start = match vm.heap.get(future_id) {
        HeapData::ExternalFuture(future) => match &future.state {
            ExternalFutureState::Pending { awaiter: None } => ExternalStart::Pending,
            ExternalFutureState::Pending { awaiter: Some(_) } => ExternalStart::Busy,
            ExternalFutureState::Resolved(value) => ExternalStart::Resolved(value.clone_with_heap(vm.heap)),
            ExternalFutureState::Failed(error) => ExternalStart::Failed(error.clone()),
        },
        _ => unreachable!("create_task accepted a non-coroutine, non-future value"),
    };

    match start {
        ExternalStart::Pending => {
            let HeapReadOutput::ExternalFuture(future) = vm.heap.read(future_id) else {
                unreachable!("create_task's future disappeared")
            };

            future.get_mut(vm.heap).state = ExternalFutureState::Pending {
                awaiter: Some(Awaiter::UserTask { task: handle }),
            };

            vm.heap.inc_ref(handle);
        }
        ExternalStart::Busy => {
            let HeapReadOutput::Task(task) = vm.heap.read(handle) else {
                unreachable!("create_task allocated a non-task handle")
            };

            task.get_mut(vm.heap).state = TaskFutureState::Failed(
                SimpleException::new_msg(ExcType::RuntimeError, "cannot reuse already awaited future").into(),
            );
        }
        ExternalStart::Resolved(value) => {
            let HeapReadOutput::Task(task) = vm.heap.read(handle) else {
                unreachable!("create_task allocated a non-task handle")
            };

            task.get_mut(vm.heap).state = TaskFutureState::Completed(value);
        }
        ExternalStart::Failed(error) => {
            let HeapReadOutput::Task(task) = vm.heap.read(handle) else {
                unreachable!("create_task allocated a non-task handle")
            };

            task.get_mut(vm.heap).state = TaskFutureState::Failed(error);
        }
    }
}

/// How `create_task` starts a host-future-backed task.
enum ExternalStart {
    /// The future is pending with no awaiter.
    Pending,
    /// Somebody else already awaits the single-awaiter future.
    Busy,
    /// Already resolved; the new task starts completed with this value.
    Resolved(Value),
    /// Already failed; the new task starts failed with this error.
    Failed(RunError),
}

/// `asyncio.create_task(coro, *, name=None)` — a Python `def`, so `coro`
/// binds positionally or by keyword and `name` is keyword-only.
#[derive(FromArgs)]
#[from_args(name = "create_task", style = def)]
struct CreateTaskArgs {
    coro: Value,
    #[from_args(kw_only, default)]
    name: Option<Value>,
}

/// Materialises a task's cached [`RunError`] as a Python exception object for
/// `Task.exception()`. Internal errors have no exception payload, so they
/// surface as a `RuntimeError` carrying the internal message.
fn exception_value(error: &RunError, vm: &mut VM<'_>) -> Value {
    let exception = match error {
        RunError::Exc(raise) | RunError::UncatchableExc(raise) => raise.exc.clone(),
        RunError::Internal(message) => SimpleException::new_msg(ExcType::RuntimeError, message),
    };
    Value::Ref(vm.heap.allocate(HeapData::Exception(exception)))
}

/// Renders a value with `str()` and extracts the resulting text.
fn value_to_string(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    let rendered = value.py_str(vm)?;
    let text = rendered.as_str(vm).map(str::to_owned).unwrap_or_default();
    rendered.drop_with(vm);
    Ok(text)
}
