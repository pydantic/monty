use std::{borrow::Cow, fmt::Write};

use monty_types::MontyUuid;

use super::{Dict, LazyHeapSet, PyTrait, Type, attribute_name_value};
use crate::{
    args::{ArgValues, KwargsValues},
    boundary_uuid::create_uuid,
    builtins::Builtins,
    bytecode::{CallResult, RunReentryGuard, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead,
        HeapReadMut, HeapReadOutput, heap_read_ref_as_field_mut,
    },
    intern::Interns,
    modules::{
        ModuleFunctions,
        copy::{Memo, PyDeepCopy, deep_copy, deep_copy_attrs},
        dataclasses,
    },
    value::{EitherStr, Value},
};

/// An instance of a user-defined class.
///
/// Holds a reference to its [`Class`](super::Class) (whose `HeapId` is the type
/// identity used by `type()`/`isinstance`) and an `attrs` [`Dict`] — the instance
/// `__dict__`. Attribute reads fall through to the class namespace for methods and
/// class variables; attribute writes only ever touch `attrs`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Instance {
    /// The class this is an instance of (a `HeapData::Class`).
    #[serde(rename = "C")]
    class: HeapId,
    /// Instance attributes (`__dict__`).
    #[serde(rename = "A")]
    attrs: Dict,
    /// Boundary identity, generated lazily the first time the instance crosses
    /// to the host; dumped with the heap so it stays stable across restores.
    #[serde(rename = "U")]
    uuid: Option<MontyUuid>,
}

impl Instance {
    /// Creates a new instance of `class` with the given initial attributes.
    #[must_use]
    pub fn new(class: HeapId, attrs: Dict) -> Self {
        Self {
            class,
            attrs,
            uuid: None,
        }
    }

    /// Boundary identity of the instance, generated and stored on first use
    /// so repeated crossings (and dump/restore) observe the same id. Only
    /// `Heap::boundary_uuid` may call this: it also indexes the new id.
    pub(crate) fn boundary_uuid(&mut self) -> MontyUuid {
        *self.uuid.get_or_insert_with(create_uuid)
    }

    /// The boundary identity, if the instance has crossed to the host.
    #[must_use]
    pub fn uuid(&self) -> Option<MontyUuid> {
        self.uuid
    }

    /// Returns the `HeapId` of the instance's class object.
    #[must_use]
    pub fn class(&self) -> HeapId {
        self.class
    }

    /// Returns a reference to the instance's attribute dict (`__dict__`).
    #[must_use]
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }
}

/// A method bound to an instance, produced by `obj.method` (without calling it).
///
/// Calling a `BoundMethod` prepends `instance` to the argument list and invokes
/// `func`. The common `obj.method()` path skips this allocation by binding and
/// calling directly in [`Instance::py_call_attr`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct BoundMethod {
    /// The bound `self` (a `Value::Ref` to the instance).
    pub instance: Value,
    /// The underlying function (`DefFunction`/`Closure`/...).
    pub func: Value,
}

impl<'h> HeapObjectRead<'h, Instance> {
    /// Dispatches class-only `__iter__` for the frozen wrapper to validate after any suspension.
    pub(crate) fn call_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let class_id = self.get(vm.heap).class();
        if let Some(member) = class_member(class_id, "__iter__", vm) {
            defer_drop!(member, vm);
            if matches!(member, Value::None) {
                Err(ExcType::type_error_not_iterable(&self.py_type_name(vm)))
            } else {
                call_member_bound(member, self.id(), ArgValues::Empty, vm)
            }
        } else {
            Err(ExcType::type_error_not_iterable(&self.py_type_name(vm)))
        }
    }

    /// Advances a user iterator through class-only lookup without imposing a synchronous boundary.
    pub(crate) fn call_next(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let class_id = self.get(vm.heap).class();
        if let Some(member) = class_member(class_id, "__next__", vm) {
            defer_drop!(member, vm);
            call_member_bound(member, self.id(), ArgValues::Empty, vm)
        } else {
            Err(ExcType::type_error_not_iterator(&self.py_type_name(vm)))
        }
    }
}

impl<'h> HeapRead<'h, Instance> {
    /// Allocates an instance of the same class as this one, with no attributes
    /// and without calling `__init__` — taking a counted reference to the class.
    ///
    /// What `copy` builds before filling a copy's `__dict__`; CPython's reducer
    /// path skips `__init__` the same way.
    pub(crate) fn allocate_empty_like(&self, vm: &mut VM<'h>) -> HeapId {
        let class_id = self.get(vm.heap).class();
        vm.heap.inc_ref(class_id);
        vm.heap
            .allocate(HeapData::Instance(Box::new(Instance::new(class_id, Dict::new()))))
    }
}

impl<'h> HeapReadMut<'h, Instance> {
    /// Borrows the inline attribute dictionary without exposing a replaceable handle.
    pub(crate) fn attrs_mut(&self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, Instance, attrs)
    }

    /// Sets an instance attribute, returning the previous value (if any) for the
    /// caller to drop. Takes ownership of both `name` and `value`.
    ///
    /// The entry point for an ordinary `obj.x = v`: it applies whatever the
    /// class has to say about the write — today just the `__setattr__` a
    /// `@dataclass(frozen=True)` generates — and is where a user `__setattr__`
    /// hook would be dispatched once one is. The write itself is
    /// [`set_attr_unchecked`](Self::set_attr_unchecked).
    pub fn set_attr(&self, name: Value, value: Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let class_id = self.get(vm.heap).class();
        if let Some(exc) = dataclasses::frozen_assignment_error(class_id, &name, vm) {
            [name, value].drop_with(vm);
            return Err(exc);
        }
        self.set_attr_unchecked(name, value, vm)
    }

    /// Writes straight to the instance `__dict__`, skipping every check
    /// [`set_attr`](Self::set_attr) makes — the only place the write itself
    /// lives, and what both `object.__setattr__` and a `frozen=True`
    /// dataclass's generated `__init__` call. The latter has to populate an
    /// instance that `set_attr` would refuse, exactly as CPython's generated
    /// `__init__` goes through `object.__setattr__`.
    ///
    /// Whatever `set_attr` grows must stay above this line (see
    /// `limitations/classes.md`).
    pub fn set_attr_unchecked(&self, name: Value, value: Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.attrs_mut().set(name, value, vm)
    }
}

/// Defines the one-argument numeric slots, sharing binding and `NotImplemented` handling.
macro_rules! instance_binary_slots {
    ($($method:ident => $dunder:literal,)*) => {
        $(
            fn $method(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
                let args = ArgValues::One(other.clone_with_heap(vm.heap));
                call_binary_dunder(self.id(), $dunder, args, vm)
            }
        )*
    };
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Instance> {
    fn py_abs_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__abs__", None, vm)
    }

    fn py_invert_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__invert__", None, vm)
    }

    fn py_round_impl(&self, ndigits: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let arg = ndigits.map(|value| value.clone_with_heap(vm.heap));
        instance_call_dunder_sync(self.id(), "__round__", arg, vm)
    }

    fn py_int_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        call_int_dunder(self.id(), "__int__", vm)
    }

    fn py_float_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<f64>> {
        if let Some(result) = instance_call_dunder_sync(self.id(), "__float__", None, vm)? {
            defer_drop!(result, vm);
            if let Value::Float(value) = result {
                Ok(Some(*value))
            } else {
                Err(ExcType::type_error(format!(
                    "{}.__float__ returned non-float (type {})",
                    self.py_type_name(vm),
                    result.py_type_name(vm),
                )))
            }
        } else {
            Ok(None)
        }
    }

    fn py_floor_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__floor__", None, vm)
    }

    fn py_ceil_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__ceil__", None, vm)
    }

    fn py_trunc_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__trunc__", None, vm)
    }

    fn py_neg_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__neg__", None, vm)
    }

    fn py_pos_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        instance_call_dunder_sync(self.id(), "__pos__", None, vm)
    }

    instance_binary_slots! {
        py_add_impl => "__add__",
        py_radd_impl => "__radd__",
        py_sub_impl => "__sub__",
        py_rsub_impl => "__rsub__",
        py_mul_impl => "__mul__",
        py_rmul_impl => "__rmul__",
        py_matmul_impl => "__matmul__",
        py_rmatmul_impl => "__rmatmul__",
        py_truediv_impl => "__truediv__",
        py_rtruediv_impl => "__rtruediv__",
        py_floordiv_impl => "__floordiv__",
        py_rfloordiv_impl => "__rfloordiv__",
        py_mod_impl => "__mod__",
        py_rmod_impl => "__rmod__",
        py_divmod_impl => "__divmod__",
        py_rdivmod_impl => "__rdivmod__",
        py_and_impl => "__and__",
        py_rand_impl => "__rand__",
        py_or_impl => "__or__",
        py_ror_impl => "__ror__",
        py_xor_impl => "__xor__",
        py_rxor_impl => "__rxor__",
        py_lshift_impl => "__lshift__",
        py_rlshift_impl => "__rlshift__",
        py_rshift_impl => "__rshift__",
        py_rrshift_impl => "__rrshift__",
    }

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        call_power_dunder(self.id(), "__pow__", other, modulus, vm)
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        call_power_dunder(self.id(), "__rpow__", other, modulus, vm)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        let key = key.clone_with_heap(vm.heap);
        instance_call_dunder_sync(self.id(), "__getitem__", Some(key), vm)?
            .ok_or_else(|| ExcType::type_error_not_sub(&self.py_type_name(vm)))
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        match call_dunder_args_sync(self.id(), "__setitem__", ArgValues::Two(key, value), vm)? {
            Some(result) => {
                result.drop_with(vm);
                Ok(())
            }
            None => Err(ExcType::type_error_not_sub_assignment(&self.py_type_name(vm))),
        }
    }

    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        // A callable instance can itself be the class's __call__; bound native
        // dispatch must be guarded even when no Python frame is pushed.
        if let Err(err) = vm.enter_run_reentry() {
            args.drop_with(vm);
            return Err(err.into());
        }
        let mut guard = RunReentryGuard::new(vm);
        let vm = &mut *guard;
        let class_id = self.get(vm.heap).class();
        if let Some(member) = class_member(class_id, "__call__", vm) {
            defer_drop!(member, vm);
            call_member_bound(member, self.id(), args, vm)
        } else {
            args.drop_with(vm);
            Err(ExcType::type_error_not_callable_object(&self.py_type_name(vm)))
        }
    }

    /// Evaluates `item in self` through the class's `__contains__`.
    ///
    /// `None` leaves the caller to fall back to iteration. An explicit
    /// `__contains__ = None` instead reports that the object is not a container.
    /// Results use `py_bool`, whose handling of user objects is documented in
    /// `limitations/classes.md`.
    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let class_id = self.get(vm.heap).class();
        if matches!(class_dunder(class_id, "__contains__", vm), Some(Value::None)) {
            Err(ExcType::type_error_object_not_container(&class_name(
                class_id, vm.heap, vm.interns,
            )))
        } else {
            // The callee owns its argument, so the borrowed `item` is cloned;
            // `instance_call_dunder_sync` drops it again if there is no `__contains__`.
            let item = item.clone_with_heap(vm.heap);
            match instance_call_dunder_sync(self.id(), "__contains__", Some(item), vm)? {
                Some(result) => {
                    defer_drop!(result, vm);
                    Ok(Some(result.py_bool(vm)?))
                }
                None => Ok(None),
            }
        }
    }

    /// Coerces the instance to an integer through the class's `__index__`.
    ///
    /// Validating the result here is both CPython's `__index__ returned non-int`
    /// check and what stops a class whose `__index__` returns another such
    /// instance from recursing — the caller converts a real int without
    /// re-entering the protocol.
    ///
    /// Runs synchronously like the other dunders here, so an `__index__` that
    /// calls an external/OS function raises rather than suspending.
    fn py_index_impl(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        call_int_dunder(self.id(), "__index__", vm)
    }

    fn py_type(&self, vm: &VM<'h>) -> Type {
        Type::Instance(self.get(vm.heap).class)
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        let mut value_guard = DropGuard::new(value, vm);
        let name = attribute_name_value(name, value_guard.ctx());
        let (value, vm) = value_guard.into_parts();
        let old_value = self.set_attr(name, value, vm)?;
        old_value.drop_with(vm);
        Ok(())
    }

    /// Returns `NotImplemented`; comparisons dispatch at the `Value` level because
    /// user and synthesized dataclass equality require the instance's `HeapId`.
    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Hashes an instance, following CPython's precedence: the class's
    /// `__hash__` is used, `__hash__ = None` is the unhashable opt-out, and
    /// defining `__eq__` is the same opt-out implicitly. What is left hashes by
    /// identity. A `@dataclass`-generated `__hash__` runs natively.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let class_id = self.get(vm.heap).class();
        if let Some(field_names) = dataclasses::generated_fields(class_id, "__hash__", vm) {
            dataclasses::dataclass_hash(self.id(), &field_names, vm).map(Some)
        } else if class_defines(class_id, "__hash__", vm) {
            if class_defines_not_none(class_id, "__hash__", vm) {
                instance_user_hash(self.id(), vm)
            } else {
                Ok(None)
            }
        } else if class_defines(class_id, "__eq__", vm) {
            // CPython's `type` sets `__hash__ = None` whenever `__eq__` is defined.
            Ok(None)
        } else {
            Ok(Some(identity_hash(self.id())))
        }
    }

    /// Dispatches to the class's `__repr__`, falling back to the
    /// identity-bearing default representation. A `@dataclass`-generated one
    /// runs natively and shares `heap_ids`, so a field pointing back renders `...`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let self_id = self.id();
        let class_id = instance_class(self_id, vm);
        if let Some(field_names) = dataclasses::generated_fields(class_id, "__repr__", vm) {
            dataclasses::dataclass_repr_fmt(self_id, &field_names, f, vm, heap_ids)
        } else if let Some(s) = instance_call_str_dunder(self_id, "__repr__", vm)? {
            defer_drop!(s, vm);
            Ok(f.write_str(s.to_str(vm)?)?)
        } else {
            self.py_default_repr_fmt(f, vm)
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);

        // 1. An instance attribute shadows class methods; call it as-is (unbound).
        if let Some(callable) = self
            .get(vm.heap)
            .attrs
            .get_by_str(attr_str, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap))
        {
            defer_drop!(callable, vm);
            return vm.call_function(callable, args);
        }

        // 2. A class member: bind `self` for methods, call data attributes as-is.
        let class_id = self.get(vm.heap).class;
        if let Some(member) = class_member(class_id, attr_str, vm) {
            defer_drop!(member, vm);
            return call_member_bound(member, self.id(), args, vm);
        }

        // 3. `obj.__class__(...)` constructs a new instance — the callable form of
        // the `obj.__class__` special attribute (see `instance_getattr` step 3).
        // Checked after the dict/namespace lookups so a same-named member wins.
        // The class value is a fresh owned ref (inc_ref) dropped by the guard once
        // `call_function` has borrowed it; `instantiate_class` takes its own ref
        // for the new instance.
        if attr_str == "__class__" {
            vm.heap.inc_ref(class_id);
            let class_val = Value::Ref(class_id);
            defer_drop!(class_val, vm);
            return vm.call_function(class_val, args);
        }

        // 4. No such attribute.
        args.drop_with(vm);
        Err(ExcType::attribute_error(
            class_name(class_id, vm.heap, vm.interns),
            attr_str,
        ))
    }

    fn py_is_iterable(&self, vm: &VM<'h>) -> bool {
        // CPython also accepts a `__getitem__`-only class here; Monty has no
        // such fallback, so it reports not-iterable (see limitations/classes.md).
        class_defines_not_none(self.get(vm.heap).class, "__iter__", vm)
    }

    fn py_is_iterator(&self, vm: &VM<'h>) -> bool {
        // Plain existence, not [`class_defines_not_none`]: `__next__ = None` is
        // not an opt-out in CPython — the class stays an iterator and calling it
        // raises "'NoneType' object is not callable".
        class_defines(self.get(vm.heap).class, "__next__", vm)
    }

    /// Validates the user method's result, including after a host suspension.
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        vm.call_frozen_exact(
            FrozenFunction::Iter,
            [
                self.clone_value(vm.heap),
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::GetIter)),
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::ValidateIterator)),
            ],
        )
    }

    /// Dispatches the class's `__next__`, turning `StopIteration` into exhaustion.
    ///
    /// Every other exception propagates, including an `UncatchableExc` from a
    /// resource limit — see [`RunError::is_stop_iteration`].
    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        match self.call_next(vm).and_then(|call| call.require_synchronous(vm)) {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.is_stop_iteration() => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn py_is_context_manager(&self, vm: &VM<'h>) -> bool {
        // CPython names `__exit__` in the protocol TypeError, so that is the
        // dunder the `BeforeWith` gate checks; a class with `__exit__` but no
        // `__enter__` passes the gate and gets the "missed __enter__ method"
        // error from `py_enter` instead — matching CPython's check order.
        class_defines(self.get(vm.heap).class, "__exit__", vm)
    }

    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let class_id = self.get(vm.heap).class;
        let Some(enter) = class_member(class_id, "__enter__", vm) else {
            return Err(ExcType::type_error_not_context_manager(
                class_name(class_id, vm.heap, vm.interns),
                "__enter__",
            ));
        };
        defer_drop!(enter, vm);
        // A plain-function `__enter__` runs as a real pushed frame
        // (`CallResult::FramePushed`), so — unlike `__repr__`/`__str__` — it
        // can suspend on external/OS calls; the frame's return value becomes
        // the `as` target via the normal `ReturnValue` push.
        call_member_bound(enter, self.id(), ArgValues::Empty, vm)
    }

    fn py_exit(&mut self, vm: &mut VM<'h>, exc: Option<HeapId>) -> RunResult<CallResult> {
        let class_id = self.get(vm.heap).class;
        let Some(exit) = class_member(class_id, "__exit__", vm) else {
            // Defensive tripwire — unreachable via `with`. `py_is_context_manager`
            // gates on `__exit__` being present at entry, and Monty has no `del`,
            // so the member cannot be removed mid-body. A reassignment (e.g. to a
            // non-callable like `None`) keeps the member present, so this branch
            // is not taken — that case fails later in `call_member_bound` as a
            // `TypeError: 'NoneType' object is not callable`.
            return Err(ExcType::attribute_error(
                class_name(class_id, vm.heap, vm.interns),
                "__exit__",
            ));
        };
        defer_drop!(exit, vm);
        // Build CPython's `(exc_type, exc_value, traceback)` triple. The type
        // is constructed as `Builtins::ExcType` — the same value the bare
        // exception name resolves to — so the idiomatic `if typ is ValueError:`
        // works inside a user `__exit__`. Monty has no traceback objects, so
        // the third slot is always `None` (see limitations/with.md).
        let (typ, val) = match exc {
            Some(exc_id) => {
                let HeapData::Exception(e) = vm.heap.get(exc_id) else {
                    // Instances only receive `Some(exc)` from `WithExceptStart`,
                    // which always passes the in-flight exception object
                    // (explicit `obj.__exit__(...)` calls go through normal
                    // method dispatch, never this trait hook).
                    unreachable!("Instance py_exit called with a non-exception heap id");
                };
                vm.heap.inc_ref(exc_id);
                (Value::Builtin(Builtins::ExcType(e.exc_type())), Value::Ref(exc_id))
            }
            None => (Value::None, Value::None),
        };
        let args = ArgValues::ArgsKargs {
            args: vec![typ, val, Value::None],
            kwargs: KwargsValues::Empty,
        };
        call_member_bound(exit, self.id(), args, vm)
    }
}

impl HeapItem for Instance {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.class);
        self.attrs.py_dec_ref_ids(stack);
    }
}

impl<'h> HeapRead<'h, BoundMethod> {
    /// Allocates a method binding this one's function to `instance`, whose
    /// reference it takes ownership of.
    ///
    /// How `copy` rebuilds a method: a fresh binding over the same `__func__`,
    /// never a copy of the function, which is shared as all functions are.
    pub(crate) fn allocate_like(&self, instance: Value, vm: &mut VM<'h>) -> Value {
        let func = self.get(vm.heap).func.clone_with_heap(vm.heap);
        Value::Ref(vm.heap.allocate(HeapData::BoundMethod(BoundMethod { instance, func })))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, BoundMethod> {
    /// Re-dispatches to the underlying function with the captured receiver
    /// pushed in front of the call's own arguments.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let bound = self.get(vm.heap);
        let instance = bound.instance.clone_with_heap(vm);
        let func = bound.func.clone_with_heap(vm);
        defer_drop!(func, vm);
        vm.call_function(func, args.prepend(instance))
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        // Monty has no dedicated `method` type; bound methods report `function`.
        Type::Function
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        // Bound methods hash by identity, consistent with their identity-only
        // equality (CPython hashes by `(instance, func)` — see limitations/classes.md).
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, _vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<bound method>")?)
    }
}

impl HeapItem for BoundMethod {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.instance.py_dec_ref_ids(stack);
        self.func.py_dec_ref_ids(stack);
    }
}

/// Reads an instance attribute for `obj.attr` (the `LoadAttr` path).
///
/// Mirrors Python's lookup order: the instance `__dict__` first, then the class
/// namespace, then the `__class__` special case. A class method becomes a
/// [`BoundMethod`] (binding `self`); a class variable is returned as-is. A missing
/// attribute raises `AttributeError` with the real class name. Takes `self_id`
/// (available at the `Value` level) because binding a method needs the instance's
/// `HeapId`.
pub(crate) fn instance_getattr(self_id: HeapId, attr: &EitherStr, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let attr_str = attr.as_str(vm.interns);
    if let Some(value) = instance_attr(self_id, attr_str, vm) {
        Ok(CallResult::Value(value))
    } else {
        let class_id = instance_class(self_id, vm);
        Err(ExcType::attribute_error(
            class_name(class_id, vm.heap, vm.interns),
            attr_str,
        ))
    }
}

/// The lookup half of [`instance_getattr`]: the instance `__dict__`, then the
/// class namespace, then the `__class__` special case; `None` when nothing binds
/// `attr`, leaving the `AttributeError` to the caller.
///
/// Split out so the synthesized dataclass `__repr__`/`__eq__` read their fields
/// exactly as `self.field` does, binding a function-valued class member as a
/// [`BoundMethod`].
pub(crate) fn instance_attr(self_id: HeapId, attr: &str, vm: &VM<'_>) -> Option<Value> {
    if let HeapReadOutput::Instance(inst) = vm.heap.read(self_id)
        && let Some(value) = inst
            .get(vm.heap)
            .attrs
            .get_by_str(attr, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap))
    {
        return Some(value);
    }
    let class_id = instance_class(self_id, vm);
    match class_member(class_id, attr, vm) {
        // A class variable is returned as-is; a function binds `self`.
        Some(member) if is_method_value(&member, vm) => {
            vm.heap.inc_ref(self_id);
            let bound = BoundMethod {
                instance: Value::Ref(self_id),
                func: member,
            };
            Some(Value::Ref(vm.heap.allocate(HeapData::BoundMethod(bound))))
        }
        Some(member) => Some(member),
        // `obj.__class__` returns the class object itself (`obj.__class__ is Foo`).
        // Last, so an explicit member of the same name wins, mirroring the
        // `__name__` handling on class objects.
        None if attr == "__class__" => {
            vm.heap.inc_ref(class_id);
            Some(Value::Ref(class_id))
        }
        None => None,
    }
}

/// Produces `repr(instance)`, dispatching to a user `__repr__` if the class
/// defines one, otherwise the default `<ClassName object at 0x..>`.
pub(crate) fn instance_repr(self_id: HeapId, vm: &mut VM<'_>) -> RunResult<Value> {
    let HeapReadOutput::Instance(instance) = vm.heap.read(self_id) else {
        unreachable!("instance_repr called on non-instance heap value")
    };
    instance.py_repr(vm)
}

/// Produces `str(instance)`, dispatching to a user `__str__` if defined, else
/// falling back to `repr` (which itself falls back to the default).
pub(crate) fn instance_str(self_id: HeapId, vm: &mut VM<'_>) -> RunResult<Value> {
    match instance_call_str_dunder(self_id, "__str__", vm)? {
        Some(s) => Ok(s),
        None => instance_repr(self_id, vm),
    }
}

/// Calls a user-defined string dunder (`__repr__`/`__str__`) on the instance and
/// validates that it returned a `str`.
///
/// Returns `Ok(None)` if the class does not define the dunder (caller uses the
/// default). The method runs to completion synchronously via `evaluate_function`,
/// so — unlike `__init__` — it cannot suspend on external/OS calls (see
/// `limitations/classes.md`). Recursion (e.g. a `__repr__` that reprs `self`)
/// re-enters the VM on the *Rust* stack; `evaluate_function`'s re-entry guard
/// bounds it with a catchable `RecursionError` — lower than CPython's depth for
/// deep-but-finite chains, a documented divergence (`limitations/classes.md`).
fn instance_call_str_dunder(self_id: HeapId, dunder: &'static str, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    let Some(result) = instance_call_dunder_sync(self_id, dunder, None, vm)? else {
        return Ok(None);
    };
    // CPython requires `__repr__`/`__str__` to return a `str`; reject any other
    // type with the same TypeError, dropping the offending return value.
    if result.is_str(vm.heap) {
        Ok(Some(result))
    } else {
        let exc = ExcType::type_error(format!(
            "{dunder} returned non-string (type {})",
            result.py_type_name(vm)
        ));
        result.drop_with(vm);
        Err(exc)
    }
}

/// Calls a zero- or one-argument dunder using class-only lookup and descriptor binding.
/// Returns `Ok(None)` if absent, consuming `arg` on every path.
/// Runs synchronously via `evaluate_function`: external/OS calls cannot suspend,
/// unlike `__call__` and `__enter__` via [`call_member_bound`].
pub(crate) fn instance_call_dunder_sync(
    self_id: HeapId,
    dunder: &'static str,
    arg: Option<Value>,
    vm: &mut VM<'_>,
) -> RunResult<Option<Value>> {
    let args = arg.map_or(ArgValues::Empty, ArgValues::One);
    call_dunder_args_sync(self_id, dunder, args, vm)
}

/// Validates integer conversion results, normalizing the permitted bool subclass.
fn call_int_dunder(self_id: HeapId, dunder: &'static str, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    match instance_call_dunder_sync(self_id, dunder, None, vm)? {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(Value::Int(i64::from(b)))),
        Some(result) => {
            if matches!(result, Value::Int(_) | Value::InternLongInt(_))
                || matches!(&result, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::LongInt(_)))
            {
                Ok(Some(result))
            } else {
                defer_drop!(result, vm);
                Err(ExcType::type_error_non_int_return(dunder, &result.py_type_name(vm)))
            }
        }
    }
}

/// Calls a synchronous special method, consuming its arguments even when absent.
fn call_dunder_args_sync(
    self_id: HeapId,
    dunder: &'static str,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<Option<Value>> {
    let class_id = instance_class(self_id, vm);
    if let Some(func) = class_member(class_id, dunder, vm) {
        call_dunder_member(self_id, func, args, vm).map(Some)
    } else {
        args.drop_with(vm);
        Ok(None)
    }
}

/// Numeric binary slots alone translate `NotImplemented` into reflected fallback.
fn call_binary_dunder(
    self_id: HeapId,
    dunder: &'static str,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<Option<Value>> {
    Ok(match call_dunder_args_sync(self_id, dunder, args, vm)? {
        Some(Value::NotImplemented) => None,
        result => result,
    })
}

/// Passes a power slot its original operands, omitting a `None` modulus.
fn call_power_dunder(
    self_id: HeapId,
    dunder: &'static str,
    other: &Value,
    modulus: Option<&Value>,
    vm: &mut VM<'_>,
) -> RunResult<Option<Value>> {
    let other = other.clone_with_heap(vm.heap);
    let args = match modulus {
        Some(modulus) => ArgValues::Two(other, modulus.clone_with_heap(vm.heap)),
        None => ArgValues::One(other),
    };
    call_binary_dunder(self_id, dunder, args, vm)
}

/// As [`instance_call_dunder_sync`], but a `None` member counts as absent.
///
/// `copy` reads its hooks with `getattr(..., None)` and calls whatever it gets
/// back only `if copier is not None`, so a class writing `__copy__ = None`
/// opts out and falls back to the ordinary attribute copy. Separate from
/// `instance_call_dunder_sync` because everywhere else a `None` member is a
/// value in its own right — a `None` `__iter__` opts a class out of iteration
/// rather than leaving it undefined (see [`instance_defines_iter`]).
pub(crate) fn instance_call_copy_hook(
    self_id: HeapId,
    dunder: &'static str,
    arg: Option<Value>,
    vm: &mut VM<'_>,
) -> RunResult<Option<Value>> {
    let class_id = instance_class(self_id, vm);
    match class_member(class_id, dunder, vm) {
        Some(Value::None) | None => {
            arg.drop_with(vm);
            Ok(None)
        }
        Some(func) => call_dunder_member(self_id, func, arg.map_or(ArgValues::Empty, ArgValues::One), vm).map(Some),
    }
}

/// Invokes an already-looked-up dunder member, whose reference it takes.
fn call_dunder_member(self_id: HeapId, func: Value, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(func, vm);
    // Only a plain function binds `self` as a descriptor (CPython's method
    // lookup protocol, mirrored by `call_member_bound`); an already-bound
    // method or other callable value is invoked without it.
    let args = if is_method_value(func, vm) {
        vm.heap.inc_ref(self_id);
        args.prepend(Value::Ref(self_id))
    } else {
        args
    };
    vm.evaluate_function(func, args)
}

/// Whether `self_id` is an instance whose class has an `__iter__` member —
/// including a `None` one, which opts the class out of iteration.
///
/// The distinction is CPython's `tp_iter`-is-non-NULL test, which only the
/// unpack error message needs (see `unpack_type_error`); everywhere else the
/// question is [`HeapObjectRead::py_is_iterable`].
pub(crate) fn instance_defines_iter(self_id: HeapId, vm: &VM<'_>) -> bool {
    match vm.heap.get(self_id) {
        HeapData::Instance(inst) => class_defines(inst.class, "__iter__", vm),
        _ => false,
    }
}

/// Whether `class_id`'s namespace defines `dunder`, without cloning it out.
///
/// Special-method lookup goes through the class only, never the instance
/// `__dict__`, matching CPython's lookup for implicit invocations. A slot whose
/// `None` value opts the class out of the protocol wants
/// [`class_defines_not_none`] instead.
pub(crate) fn class_defines(class_id: HeapId, dunder: &str, vm: &VM<'_>) -> bool {
    class_dunder(class_id, dunder, vm).is_some()
}

/// Whether `class_id` defines `dunder` as something other than `None`.
///
/// `__iter__ = None` and `__contains__ = None` are explicit protocol opt-outs:
/// CPython's `slot_tp_iter` / `slot_sq_contains` reject a `None` member with the
/// same error as an absent one rather than calling it. This is per-slot, not
/// general — `__next__ = None` keeps the class an iterator (see
/// [`HeapObjectRead::py_is_iterator`]), so use [`class_defines`] there.
pub(crate) fn class_defines_not_none(class_id: HeapId, dunder: &str, vm: &VM<'_>) -> bool {
    matches!(class_dunder(class_id, dunder, vm), Some(member) if !matches!(member, Value::None))
}

/// Borrows a dunder out of `class_id`'s namespace, or `None` if absent.
///
/// Backs the existence checks above without the `clone_with_heap` that
/// [`class_member`] pays to hand out an owned value. Callers needing to tell a
/// `None` member apart from an absent one — CPython's `has_explicit_hash` does
/// — want this rather than either check.
pub(crate) fn class_dunder<'v>(class_id: HeapId, dunder: &str, vm: &'v VM<'_>) -> Option<&'v Value> {
    namespace_member(class_id, dunder, vm.heap, vm.interns)
}

/// [`class_dunder`] from a bare heap and interns, for a dump upgrade that runs
/// before any VM exists.
pub(crate) fn namespace_member<'v>(
    class_id: HeapId,
    name: &str,
    heap: &'v Heap,
    interns: &Interns,
) -> Option<&'v Value> {
    match heap.get(class_id) {
        HeapData::Class(class) => class.namespace().get_by_str(name, heap, interns),
        _ => None,
    }
}

/// Returns the `HeapId` of `self_id`'s class object.
fn instance_class(self_id: HeapId, vm: &VM<'_>) -> HeapId {
    match vm.heap.get(self_id) {
        HeapData::Instance(inst) => inst.class,
        _ => unreachable!("instance_class called on non-instance heap value"),
    }
}

/// Dispatches the class's `__eq__`, or `Ok(None)` when it is absent.
///
/// The user's value is preserved so direct equality can return it unchanged;
/// callers interpret `NotImplemented` according to their comparison mode. A
/// `@dataclass`-generated `__eq__` runs natively rather than through a call.
pub(crate) fn instance_user_eq(self_id: HeapId, other: &Value, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    if !matches!(vm.heap.get(self_id), HeapData::Instance(_)) {
        return Ok(None);
    }
    let class_id = instance_class(self_id, vm);
    if let Some(field_names) = dataclasses::generated_fields(class_id, "__eq__", vm) {
        let equal = dataclasses::dataclass_eq(self_id, &field_names, other, vm)?;
        Ok(Some(equal.map_or(Value::NotImplemented, Value::Bool)))
    } else if class_defines(class_id, "__eq__", vm) {
        let other = other.clone_with_heap(vm.heap);
        instance_call_dunder_sync(self_id, "__eq__", Some(other), vm)
    } else {
        Ok(None)
    }
}

/// Dispatches a user-defined `__hash__`, enforcing CPython's integer-return
/// contract. Only reached once the class is known to define a non-`None`
/// `__hash__`, so an absent member is an internal error.
fn instance_user_hash(self_id: HeapId, vm: &mut VM<'_>) -> RunResult<Option<HashValue>> {
    let Some(result) = instance_call_dunder_sync(self_id, "__hash__", None, vm)? else {
        return Err(RunError::internal(
            "instance_user_hash: __hash__ vanished from the class",
        ));
    };
    defer_drop!(result, vm);
    // CPython accepts any int (bool included, as an int subclass) and rejects
    // everything else before the value is ever used as a hash.
    if matches!(result.py_type(vm), Type::Int | Type::Bool) {
        result.py_hash(vm)
    } else {
        Err(ExcType::type_error(format!(
            "__hash__ method should return an integer, not {}",
            result.py_type_name(vm)
        )))
    }
}

/// Looks up a member in a class namespace and clones it out, or `None` if absent.
fn class_member(class_id: HeapId, name: &str, vm: &VM<'_>) -> Option<Value> {
    match vm.heap.get(class_id) {
        HeapData::Class(class) => class
            .namespace()
            .get_by_str(name, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap)),
        _ => None,
    }
}

/// Returns a class object's name for error messages / repr.
///
/// Takes `heap` + `interns` rather than a `&VM` so heap-only contexts (e.g.
/// `Type::name`) can resolve names. The result borrows only the interner —
/// interned names are `Cow::Borrowed`, while heap-owned names (classes created
/// by the 3-arg `type()` form) are cloned into `Cow::Owned` here, so either way
/// the result survives subsequent heap mutation.
///
/// # Panics
/// If `class_id` does not refer to a `Class` heap entry — every producer of a
/// class id (`Instance.class`, class values) guarantees it does, so this is a
/// programmer-error tripwire.
pub(crate) fn class_name<'i>(class_id: HeapId, heap: &Heap, interns: &'i Interns) -> Cow<'i, str> {
    match heap.get(class_id) {
        HeapData::Class(class) => match class.name() {
            EitherStr::Interned(id) => Cow::Borrowed(interns.get_str(*id)),
            EitherStr::Heap(s) => Cow::Owned(s.clone()),
        },
        _ => unreachable!("class_name called with a non-class heap id"),
    }
}

/// Calls a class member with CPython's descriptor-binding semantics: a
/// plain user-defined function binds `self` (prepended to `args`), while any
/// other callable value is called as-is. Shared by `py_call_attr` and the
/// context-manager and iterator hooks so special-method invocation and
/// ordinary method calls dispatch identically.
fn call_member_bound(member: &Value, self_id: HeapId, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    if is_method_value(member, vm) {
        vm.heap.inc_ref(self_id);
        vm.call_function(member, args.prepend(Value::Ref(self_id)))
    } else {
        vm.call_function(member, args)
    }
}

/// Whether a value is a user-defined function (so it should bind `self` when
/// accessed as a method). Class variables that are not functions are returned
/// unbound.
fn is_method_value(value: &Value, vm: &VM<'_>) -> bool {
    match value {
        Value::DefFunction(_) => true,
        Value::Ref(id) => matches!(
            vm.heap.get(*id),
            // `functools.partial` and the `lru_cache` wrapper are descriptors in
            // CPython 3.14 too, and bind the instance the same way: after the
            // arguments they already carry. A `@dataclass`-generated method is a
            // function in CPython.
            HeapData::Closure(_)
                | HeapData::FunctionDefaults(_)
                | HeapData::Partial(_)
                | HeapData::LruCache(_)
                | HeapData::DataclassMethod(_)
        ),
        _ => false,
    }
}

impl<'h> PyDeepCopy<'h> for HeapRead<'h, Instance> {
    /// Copies a class instance: `__deepcopy__` when the class defines one, else
    /// a new instance with a deep-copied `__dict__` and no `__init__` call —
    /// which is what CPython's reducer path does too.
    #[inline(never)]
    fn py_deep_copy(&self, source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        let id = source.ref_id().expect("a deep copy starts from a heap value");
        if let Some(copy) = instance_call_copy_hook(id, "__deepcopy__", Some(memo.dict_value(vm)), vm)? {
            Ok(copy)
        } else {
            let copy_id = self.allocate_empty_like(vm);
            deep_copy_attrs(id, copy_id, source, memo, vm)
        }
    }
}

impl<'h> PyDeepCopy<'h> for HeapRead<'h, BoundMethod> {
    /// Copies a bound method by rebinding its function to a deep copy of its
    /// receiver — CPython's `_deepcopy_method`.
    ///
    /// The receiver goes through the memo, so copying an object and one of its
    /// methods together binds the copy to the copied object, and an instance
    /// holding its own method terminates on the shell.
    #[inline(never)]
    fn py_deep_copy(&self, _source: &Value, memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        let instance = self.get(vm.heap).instance.clone_with_heap(vm.heap);
        let copied = deep_copy(&instance, memo, vm);
        instance.drop_with(vm);
        Ok(self.allocate_like(copied?, vm))
    }
}
