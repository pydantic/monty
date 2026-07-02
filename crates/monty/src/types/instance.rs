use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
    mem,
};

use ahash::AHashSet;

use super::{Dict, PyTrait, Type};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    hash::HashValue,
    heap::{
        BorrowedHeapReadMut, DropWithHeap, HeapData, HeapId, HeapItem, HeapRead, HeapReadOutput,
        heap_read_ref_as_field_mut,
    },
    resource::ResourceTracker,
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
    class: HeapId,
    /// Instance attributes (`__dict__`).
    attrs: Dict,
}

impl Instance {
    /// Creates a new instance of `class` with the given initial attributes.
    #[must_use]
    pub fn new(class: HeapId, attrs: Dict) -> Self {
        Self { class, attrs }
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

impl<'h> HeapRead<'h, Instance> {
    fn attrs_mut(&mut self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, Instance, attrs)
    }

    /// Sets an instance attribute, returning the previous value (if any) for the
    /// caller to drop. Takes ownership of both `name` and `value`.
    pub fn set_attr(
        &mut self,
        name: Value,
        value: Value,
        vm: &mut VM<'h, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        self.attrs_mut().set(name, value, vm)
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Instance> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        Type::Instance
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        // Instances are always truthy (no user `__bool__`/`__len__` in v1).
        true
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        // Identity equality, resolved by `Value::py_eq_impl` before reaching here.
        Ok(None)
    }

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        // Instances hash by identity (CPython's default for objects without `__hash__`).
        let mut hasher = DefaultHasher::new();
        self_id.hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    /// Heap-level `repr` fallback.
    ///
    /// Real `repr()`/`str()` (including dispatch to a user `__repr__`/`__str__`)
    /// is handled at the `Value` level — see [`instance_repr`] / [`instance_str`] —
    /// because it needs the instance's `HeapId` to pass `self`, which this method
    /// does not receive. This produces a best-effort default and is essentially
    /// never reached.
    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        let class_id = self.get(vm.heap).class;
        Ok(write!(f, "<{} object>", class_name(class_id, vm))?)
    }

    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
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
            return if is_method_value(member, vm) {
                vm.heap.inc_ref(self_id);
                vm.call_function(member, args.prepend(Value::Ref(self_id)))
            } else {
                vm.call_function(member, args)
            };
        }

        // 3. No such attribute.
        args.drop_with_heap(vm);
        Err(ExcType::attribute_error(class_name(class_id, vm), attr_str))
    }
}

impl HeapItem for Instance {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>() + self.attrs.py_estimate_size()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.push(self.class);
        self.attrs.py_dec_ref_ids(stack);
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, BoundMethod> {
    fn py_type(&self, _vm: &VM<'h, impl ResourceTracker>) -> Type {
        // Monty has no dedicated `method` type; bound methods report `function`.
        Type::Function
    }

    fn py_len(&self, _vm: &VM<'h, impl ResourceTracker>) -> Option<usize> {
        None
    }

    fn py_bool(&self, _vm: &mut VM<'h, impl ResourceTracker>) -> bool {
        true
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h, impl ResourceTracker>) -> RunResult<Option<HashValue>> {
        // Bound methods hash by identity, consistent with their identity-only
        // equality (CPython hashes by `(instance, func)` — see limitations/classes.md).
        let mut hasher = DefaultHasher::new();
        self_id.hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _vm: &mut VM<'h, impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
    ) -> RunResult<()> {
        Ok(write!(f, "<bound method>")?)
    }
}

impl HeapItem for BoundMethod {
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.instance.py_dec_ref_ids(stack);
        self.func.py_dec_ref_ids(stack);
    }
}

/// Reads an instance attribute for `obj.attr` (the `LoadAttr` path).
///
/// Mirrors Python's lookup order: the instance `__dict__` first, then the class
/// namespace. A class method becomes a [`BoundMethod`] (binding `self`); a class
/// variable is returned as-is. A missing attribute raises `AttributeError` with
/// the real class name. Takes `self_id` (available at the `Value` level) because
/// binding a method needs the instance's `HeapId`.
pub(crate) fn instance_getattr(
    self_id: HeapId,
    attr: &EitherStr,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<CallResult> {
    let attr_str = attr.as_str(vm.interns);

    // 1. Instance dict.
    if let HeapReadOutput::Instance(inst) = vm.heap.read(self_id)
        && let Some(value) = inst
            .get(vm.heap)
            .attrs
            .get_by_str(attr_str, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap))
    {
        return Ok(CallResult::Value(value));
    }

    // 2. Class namespace: bind methods, return class variables as-is.
    let class_id = instance_class(self_id, vm);
    if let Some(member) = class_member(class_id, attr_str, vm) {
        if is_method_value(&member, vm) {
            vm.heap.inc_ref(self_id);
            let bound = BoundMethod {
                instance: Value::Ref(self_id),
                func: member,
            };
            let id = vm.heap.allocate(HeapData::BoundMethod(bound))?;
            Ok(CallResult::Value(Value::Ref(id)))
        } else {
            Ok(CallResult::Value(member))
        }
    } else {
        Err(ExcType::attribute_error(class_name(class_id, vm), attr_str))
    }
}

/// Produces `repr(instance)`, dispatching to a user `__repr__` if the class
/// defines one, otherwise the default `<ClassName object at 0x..>`.
pub(crate) fn instance_repr(self_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
    match instance_call_str_dunder(self_id, "__repr__", vm)? {
        Some(s) => Ok(s),
        None => Ok(Cow::Owned(default_repr(self_id, vm))),
    }
}

/// Produces `str(instance)`, dispatching to a user `__str__` if defined, else
/// falling back to `repr` (which itself falls back to the default).
pub(crate) fn instance_str(self_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> RunResult<Cow<'static, str>> {
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
/// `limitations/classes.md`). NOTE: recursion (e.g. a `__repr__` that reprs
/// `self`) re-enters the VM on the *Rust* stack and is currently NOT bounded
/// before the native stack overflows — a pre-existing `evaluate_function` issue
/// tracked in recursion_error.md.
fn instance_call_str_dunder(
    self_id: HeapId,
    dunder: &'static str,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Option<Cow<'static, str>>> {
    let class_id = instance_class(self_id, vm);
    let Some(func) = class_member(class_id, dunder, vm) else {
        return Ok(None);
    };
    defer_drop!(func, vm);
    vm.heap.inc_ref(self_id);
    let result = vm.evaluate_function(dunder, func, ArgValues::One(Value::Ref(self_id)))?;
    value_into_string(result, dunder, vm).map(Some)
}

/// Converts a string-dunder return value into an owned string, raising `TypeError`
/// if it is not a `str`. Consumes (drops) `value`.
fn value_into_string(
    value: Value,
    dunder: &str,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<Cow<'static, str>> {
    let extracted = match &value {
        Value::InternString(id) => Some(vm.interns.get_str(*id).to_owned()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Str(s) => Some(s.as_str().to_owned()),
            _ => None,
        },
        _ => None,
    };
    let type_name = value.py_type(vm);
    value.drop_with_heap(vm);
    match extracted {
        Some(s) => Ok(Cow::Owned(s)),
        None => Err(ExcType::type_error(format!(
            "{dunder} returned non-string (type {type_name})"
        ))),
    }
}

/// The default `repr` for an instance with no user `__repr__`.
fn default_repr(self_id: HeapId, vm: &mut VM<'_, impl ResourceTracker>) -> String {
    let class_id = instance_class(self_id, vm);
    format!("<{} object at 0x{:x}>", class_name(class_id, vm), self_id.index())
}

/// Returns the `HeapId` of `self_id`'s class object.
fn instance_class(self_id: HeapId, vm: &VM<'_, impl ResourceTracker>) -> HeapId {
    match vm.heap.get(self_id) {
        HeapData::Instance(inst) => inst.class,
        _ => unreachable!("instance_class called on non-instance heap value"),
    }
}

/// Looks up a member in a class namespace and clones it out, or `None` if absent.
fn class_member(class_id: HeapId, name: &str, vm: &VM<'_, impl ResourceTracker>) -> Option<Value> {
    match vm.heap.get(class_id) {
        HeapData::Class(class) => class
            .namespace()
            .get_by_str(name, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap)),
        _ => None,
    }
}

/// Returns a class object's name as a string slice for error messages / repr.
fn class_name<'a>(class_id: HeapId, vm: &'a VM<'_, impl ResourceTracker>) -> &'a str {
    match vm.heap.get(class_id) {
        HeapData::Class(class) => vm.interns.get_str(class.name_id()),
        _ => "object",
    }
}

/// Whether a value is a user-defined function (so it should bind `self` when
/// accessed as a method). Class variables that are not functions are returned
/// unbound.
fn is_method_value(value: &Value, vm: &VM<'_, impl ResourceTracker>) -> bool {
    match value {
        Value::DefFunction(_) => true,
        Value::Ref(id) => matches!(vm.heap.get(*id), HeapData::Closure(_) | HeapData::FunctionDefaults(_)),
        _ => false,
    }
}
