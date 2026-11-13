use std::fmt::Write;

use monty_types::MontyUuid;

use super::{Dict, LazyHeapSet, PyTrait, Type, descriptor};
use crate::{
    annotations,
    args::ArgValues,
    boundary_uuid::create_uuid,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, DropGuard, DropWithContext, Heap, HeapId, HeapItem, HeapObjectRead, HeapReadMut,
        HeapReader, heap_read_ref_as_field_mut,
    },
    intern::{Interns, StringId},
    types::{Union, str::allocate_string},
    value::{EitherStr, Value},
};

/// A user-defined class object created by a `class Foo: ...` statement.
///
/// Holds the class name and a `namespace` [`Dict`] mapping member names to values:
/// methods (stored as `DefFunction`/`Closure` values) and class variables. The
/// class's own [`HeapId`] is its type identity — `type(x) is Foo` and `isinstance`
/// work via reference identity, so there is no separate type-id counter.
///
/// Calling a class (`Foo(...)`) constructs an [`Instance`](super::Instance); see
/// `instantiate_class` in the VM's call module. Inheritance is not yet supported,
/// but a future `bases: Vec<HeapId>` field would slot in here without disturbing
/// the rest of the design.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Class {
    /// Class name (e.g. `Foo`), used for `repr` and `__name__`. Interned for
    /// compiled `class` statements; heap-owned for classes created at runtime
    /// via the 3-arg `type(name, bases, dict)` form, whose name cannot be
    /// interned because the intern table is frozen after prepare.
    name: EitherStr,
    /// Members: method name / class-variable name -> value.
    namespace: Dict,
    /// Boundary identity, generated lazily the first time the class (or one of
    /// its instances) crosses to the host; dumped with the heap so it stays
    /// stable across restores.
    uuid: Option<MontyUuid>,
    /// No member needs descriptor-hook dispatch; native function binding is handled separately.
    /// Missing hints in older dumps default to false, retaining full precedence checks.
    #[serde(default)]
    descriptor_free: bool,
    /// Instance storage can be checked first, even with builtin non-data descriptors present.
    /// User-defined members remain candidates because their types can acquire setter/deleter hooks.
    #[serde(default)]
    data_descriptor_free: bool,
}

impl Class {
    /// Creates a new class object from its name and member namespace.
    #[must_use]
    pub fn new(name: EitherStr, namespace: Dict, heap: &Heap) -> Self {
        let descriptor_free = namespace
            .iter()
            .all(|(_, value)| !descriptor::may_be_descriptor(value, heap));
        let data_descriptor_free = namespace
            .iter()
            .all(|(_, value)| !descriptor::may_be_data_descriptor(value, heap));
        Self {
            name,
            namespace,
            uuid: None,
            descriptor_free,
            data_descriptor_free,
        }
    }

    /// No stored member can require descriptor-hook dispatch without a namespace write.
    pub(crate) fn descriptor_free(&self) -> bool {
        self.descriptor_free
    }

    /// No stored member can become a data descriptor without a namespace write.
    pub(crate) fn data_descriptor_free(&self) -> bool {
        self.data_descriptor_free
    }

    /// Boundary identity of the class, generated and stored on first use so
    /// repeated crossings (and dump/restore) observe the same id. Only
    /// `Heap::boundary_uuid` may call this: it also indexes the new id.
    pub(crate) fn boundary_uuid(&mut self) -> MontyUuid {
        *self.uuid.get_or_insert_with(create_uuid)
    }

    /// The boundary identity, if the class (or an instance) has crossed to the host.
    #[must_use]
    pub fn uuid(&self) -> Option<MontyUuid> {
        self.uuid
    }

    /// Returns the class name (interned or heap-owned).
    #[must_use]
    pub fn name(&self) -> &EitherStr {
        &self.name
    }

    /// Returns a reference to the class member namespace.
    #[must_use]
    pub fn namespace(&self) -> &Dict {
        &self.namespace
    }
}

impl<'h> HeapReadMut<'h, Class> {
    /// Borrows the inline namespace for reader-gated attribute mutation.
    fn namespace_mut(&self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, Class, namespace)
    }

    /// Sets a class attribute (`Foo.x = 1`), returning the previous value (if any)
    /// for the caller to drop. Takes ownership of both `name` and `value`.
    ///
    /// Existing instances observe the change immediately: instance attribute reads
    /// fall through to this namespace.
    pub fn set_attr(&self, name: Value, value: Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        // Clear hints before publishing a candidate, including instances whose types have no hooks yet.
        // Re-enabling after replacement/deletion would require rescanning all remaining candidates.
        if descriptor::may_be_descriptor(&value, vm.heap) {
            self.get_mut(vm.heap).descriptor_free = false;
        }
        if descriptor::may_be_data_descriptor(&value, vm.heap) {
            self.get_mut(vm.heap).data_descriptor_free = false;
        }
        self.namespace_mut().set(name, value, vm)
    }

    /// [`set_attr`](Self::set_attr) for a dump upgrade, which has no VM and
    /// cannot fail: see `HeapRead::<Dict>::set_interned_str`.
    pub(crate) fn set_interned_attr(
        &self,
        name: StringId,
        value: Value,
        heap: &mut HeapReader<'h>,
        interns: &Interns,
    ) -> Option<Value> {
        // Dump upgrades currently write native methods; keep other future writers conservative too.
        if descriptor::may_be_descriptor(&value, heap) {
            self.get_mut(heap).descriptor_free = false;
        }
        if descriptor::may_be_data_descriptor(&value, heap) {
            self.get_mut(heap).data_descriptor_free = false;
        }
        self.namespace_mut().set_interned_str(name, value, heap, interns)
    }

    /// Removes a class attribute, returning it for the caller to drop.
    ///
    /// Only `@dataclass` needs this, to clear the `field()` object a class body
    /// bound for a factory-only field — CPython's decorator `delattr`s it.
    pub fn pop_attr(&self, name: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(self.namespace_mut().pop(name, vm)?.map(|(key, value)| {
            key.drop_with(vm);
            value
        }))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Class> {
    /// Constructing an instance, which runs `__init__` as an ordinary frame.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        vm.instantiate_class(self.id(), args)
    }

    fn py_type(&self, _vm: &VM<'h>) -> Type {
        // The type of a class object is `type` (matching `type(Foo) is type`).
        Type::Type
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_or(self, other, vm)
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Union::heap_ror(self, other, vm)
    }

    /// `Foo[int]`: a class's `__class_getitem__` is never looked up, so the
    /// wording is CPython's for a type without one.
    fn py_getitem(&self, _key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        Err(ExcType::type_error_type_not_subscriptable(
            self.get(vm.heap).name.as_str(vm.interns),
        ))
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        if name.as_str(vm.interns) == "__annotations__" {
            return annotations::set_class_annotations(self.id(), value, vm);
        }
        let mut value_guard = DropGuard::new(value, vm);
        let name = name.as_value(value_guard.ctx().heap);
        let (value, vm) = value_guard.into_parts();
        let old_value = self.set_attr(name, value, vm)?;
        old_value.drop_with(vm);
        Ok(())
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // Classes return `NotImplemented`; rich equality's final identity
        // fallback makes a class equal only to itself.
        Ok(None)
    }

    fn py_hash(&self, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        // Class objects hash by identity (like CPython type objects).
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(f, "<class '{}'>", self.get(vm.heap).name.as_str(vm.interns))?)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_str = attr.as_str(vm.interns);
        // `Foo.__name__` returns the class name — before the namespace lookup
        // because in CPython `type.__name__` is a metaclass data descriptor that
        // shadows a same-named class-dict member (`class Foo: __name__ = 'bar'`
        // still reads `'Foo'`; only instances see the member).
        if attr_str == "__name__" {
            let name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
            return Ok(Some(CallResult::Value(allocate_string(name, vm.heap))));
        }
        // `Foo.__annotations__` is computed from the class's `__annotate__` on
        // first access (PEP 649), another metaclass descriptor in CPython.
        if attr_str == "__annotations__" {
            return annotations::class_annotations(self.id(), vm).map(Some);
        }
        match descriptor::resolve_class(self.id(), attr_str, vm) {
            descriptor::AttributeResolution::Getter(value) => {
                let value = value.clone_with_heap(vm.heap);
                defer_drop!(value, vm);
                descriptor::get(value, None, self.id(), vm).map(Some)
            }
            resolution => resolution
                .into_value(self.id(), vm)
                .map(|value| Some(CallResult::Value(value)))
                .ok_or_else(|| ExcType::attribute_error_type(self.get(vm.heap).name.as_str(vm.interns), attr_str)),
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let mut guard = DropGuard::new(args, vm);
        let (_, vm) = guard.as_parts_mut();
        // Synthesized metaclass attributes must follow the same read path as LoadAttr.
        if !matches!(attr.as_str(vm.interns), "__name__" | "__annotations__")
            && matches!(
                descriptor::resolve_class(self.id(), attr.as_str(vm.interns), vm),
                descriptor::AttributeResolution::Getter(_)
            )
        {
            let (args, vm) = guard.into_parts();
            vm.heap.inc_ref(self.id());
            descriptor::call_attr(Value::Ref(self.id()), attr, args, vm)
        } else {
            let result = self
                .py_getattr(attr, vm)?
                .expect("class read returns a result or raises");
            let callable = result.require_synchronous(vm)?;
            let (args, vm) = guard.into_parts();
            defer_drop!(callable, vm);
            vm.call_function(callable, args)
        }
    }
}

impl HeapItem for Class {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.namespace.py_dec_ref_ids(stack);
    }
}
