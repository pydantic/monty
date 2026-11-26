//! Single-threaded locks for synchronous Python code.

use num_traits::ToPrimitive;
use serde::{Deserialize, Serialize};

use super::{ModuleFunctions, time::seconds_from_object};
use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    types::{BoundMethod, Module, PyTrait, Type, TypeRef},
    value::{EitherStr, Value},
};

/// Exposes the primitive lock class and the reentrant lock factory.
pub(super) fn create_module(vm: &mut VM<'_>) -> Module {
    let mut module = Module::new(StaticStrings::Threading, vm.interns);
    module.set_attr(StaticStrings::Lock, Value::Builtin(Builtins::Type(Type::Lock)), vm);
    module.set_attr(StaticStrings::RLock, ThreadingFunction::RLock.value(), vm);
    module
}

/// Both lock kinds have one possible owner in a Monty session.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Lock {
    Plain(bool),
    Reentrant(u64),
}

impl Lock {
    /// Both Python types share the same heap payload.
    pub(crate) fn lock_type(&self) -> Type {
        match self {
            Self::Plain(_) => Type::Lock,
            Self::Reentrant(_) => Type::RLock,
        }
    }

    /// A reentrant lock remains held until its last release.
    fn locked(&self) -> bool {
        match self {
            Self::Plain(held) => *held,
            Self::Reentrant(depth) => *depth != 0,
        }
    }

    /// A finite wait on a held lock expires immediately. An indefinite wait would deadlock.
    fn acquire(&mut self, wait_forever: bool) -> RunResult<Value> {
        match self {
            Self::Plain(true) if wait_forever => {
                return Err(
                    SimpleException::new_msg(ExcType::RuntimeError, "cannot wait for a locked lock in Monty").into(),
                );
            }
            Self::Plain(true) => return Ok(Value::Bool(false)),
            Self::Plain(held) => *held = true,
            Self::Reentrant(depth) => {
                *depth = depth
                    .checked_add(1)
                    .ok_or_else(|| ExcType::overflow_error("Internal lock count overflowed"))?;
            }
        }
        Ok(Value::Bool(true))
    }

    /// Releases one acquisition, raising on an unmatched release.
    fn release(&mut self) -> RunResult<Value> {
        match self {
            Self::Plain(false) => Err(SimpleException::new_msg(ExcType::RuntimeError, "release unlocked lock").into()),
            Self::Reentrant(0) => {
                Err(SimpleException::new_msg(ExcType::RuntimeError, "cannot release un-acquired lock").into())
            }
            Self::Plain(held) => {
                *held = false;
                Ok(Value::None)
            }
            Self::Reentrant(depth) => {
                *depth -= 1;
                Ok(Value::None)
            }
        }
    }
}

/// Constructs the primitive lock exposed as `threading.Lock`.
pub(crate) fn new_lock(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("Lock", vm.heap)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Lock(Lock::Plain(false)))))
}

/// Constructs a reentrant lock for the public factory and the native type.
pub(crate) fn new_rlock(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("RLock", vm.heap)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::Lock(Lock::Reentrant(0)))))
}

/// Callable stored in the module or captured by a bound lock method.
#[monty_macros::explicit_discriminants(checksum = 0x2ece_d551_4e5a_063b)]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, Serialize, Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ThreadingFunction {
    #[strum(serialize = "RLock")]
    RLock = 0,
    Acquire = 1,
    Release = 2,
    Locked = 3,
}

impl ThreadingFunction {
    /// Resolves the public instance methods shared by both lock kinds.
    fn method(attr: &EitherStr, vm: &VM<'_>) -> Option<Self> {
        match attr.static_string(vm.interns)? {
            StaticStrings::Acquire => Some(Self::Acquire),
            StaticStrings::Release => Some(Self::Release),
            StaticStrings::Locked => Some(Self::Locked),
            _ => None,
        }
    }

    /// Materializes the callable used by module and instance attribute lookup.
    fn value(self) -> Value {
        Value::ModuleFunction(ModuleFunctions::Threading(self))
    }
}

/// A bound call prepends its lock to the public `acquire(blocking=True, timeout=-1)` arguments.
#[derive(FromArgs)]
#[from_args(name = "acquire")]
struct AcquireArgs {
    #[from_args(pos_only)]
    lock: Value,
    #[from_args(default = LaxBool::new(true))]
    blocking: LaxBool,
    #[from_args(default = Value::Int(-1))]
    timeout: Value,
}

/// Whether acquisition would wait indefinitely on CPython.
fn wait_forever(blocking: LaxBool, timeout: &Value, vm: &mut VM<'_>) -> RunResult<bool> {
    let seconds = seconds_from_object(timeout, vm)?;
    if seconds.is_nan() {
        return Err(ExcType::invalid_nan());
    }
    // CPython rounds negative timeouts down to nanoseconds before testing for the -1 second sentinel.
    let nanos = (seconds * 1e9).floor().to_i64().ok_or_else(ExcType::sleep_too_long)?;
    let indefinite = nanos == -1_000_000_000;
    if !blocking.bool() && !indefinite {
        return Err(ExcType::value_error("can't specify a timeout for a non-blocking call"));
    }
    if seconds < 0.0 && !indefinite {
        return Err(ExcType::value_error("timeout value must be a non-negative number"));
    }
    Ok(blocking.bool() && indefinite)
}

/// Dispatches both immediate method calls and calls through a saved bound method.
pub(super) fn call(vm: &mut VM<'_>, function: ThreadingFunction, args: ArgValues) -> RunResult<Value> {
    match function {
        ThreadingFunction::RLock => new_rlock(vm, args),
        ThreadingFunction::Acquire => {
            let AcquireArgs {
                lock,
                blocking,
                timeout,
            } = AcquireArgs::from_args(args, vm)?;
            defer_drop!(lock, vm);
            defer_drop!(timeout, vm);
            let wait = wait_forever(blocking, timeout, vm)?;
            state(lock, vm)?.get_mut(vm.heap).acquire(wait)
        }
        ThreadingFunction::Release => {
            let lock = args.get_one_arg("release", vm.heap)?;
            defer_drop!(lock, vm);
            state(lock, vm)?.get_mut(vm.heap).release()
        }
        ThreadingFunction::Locked => {
            let lock = args.get_one_arg("locked", vm.heap)?;
            defer_drop!(lock, vm);
            Ok(Value::Bool(state(lock, vm)?.get(vm.heap).locked()))
        }
    }
}

/// Borrows the receiver after argument coercion, which can reenter Python.
fn state<'h>(lock: &Value, vm: &VM<'h>) -> RunResult<HeapObjectRead<'h, Lock>> {
    match lock.ref_id().map(|id| vm.heap.read(id)) {
        Some(HeapReadOutput::Lock(state)) => Ok(state),
        _ => Err(ExcType::type_error("expected a lock")),
    }
}

impl HeapItem for Lock {
    fn py_dec_ref_ids(&mut self, _: &mut Vec<HeapId>) {}
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Lock> {
    fn py_type(&self, vm: &VM<'h>) -> TypeRef {
        self.get(vm.heap).lock_type().into()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        Ok(ThreadingFunction::method(attr, vm)
            .map(|func| CallResult::Value(BoundMethod::allocate(self.clone_value(vm.heap), func.value(), vm.heap))))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        if let Some(func) = ThreadingFunction::method(attr, vm) {
            call(vm, func, args.prepend(self.clone_value(vm.heap))).map(CallResult::Value)
        } else {
            args.drop_with(vm);
            Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns)))
        }
    }

    fn py_is_context_manager(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        self.get_mut(vm.heap).acquire(true).map(CallResult::Value)
    }

    fn py_exit(&mut self, vm: &mut VM<'h>, _: Option<HeapId>) -> RunResult<CallResult> {
        self.get_mut(vm.heap).release().map(CallResult::Value)
    }
}
