use std::fmt::Write;

use monty_types::MontyUuid;

use super::{Dict, LazyHeapSet, List, PyTrait, Type, attribute_name_value, tuple::allocate_tuple};
use crate::{
    args::ArgValues,
    boundary_uuid::create_uuid,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, DropGuard, DropWithContext, Heap, HeapId, HeapItem, HeapObjectRead, HeapReadMut,
        HeapReader, heap_read_ref_as_field_mut,
    },
    heap_data::HeapData,
    intern::{FunctionId, Interns, StringId},
    types::{Union, str::allocate_string},
    value::{EitherStr, Value},
};

/// An entry in a class's direct bases or C3 method resolution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum ClassBase {
    Class(HeapId),
    BuiltinExc(ExcType),
    Object,
}

/// A user-defined class object created by a `class Foo: ...` statement.
///
/// Holds the class name and a `namespace` [`Dict`] mapping member names to values:
/// methods (stored as `DefFunction`/`Closure` values) and class variables. The
/// class's own [`HeapId`] is its type identity — `type(x) is Foo` and `isinstance`
/// work via reference identity, so there is no separate type-id counter.
///
/// Calling a class (`Foo(...)`) constructs an [`Instance`](super::Instance); see
/// `instantiate_class` in the VM's call module.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Class {
    /// Class name (e.g. `Foo`), used for `repr` and `__name__`. Interned for
    /// compiled `class` statements; heap-owned for classes created at runtime
    /// via the 3-arg `type(name, bases, dict)` form, whose name cannot be
    /// interned because the intern table is frozen after prepare.
    name: EitherStr,
    /// Members: method name / class-variable name -> value.
    namespace: Dict,
    #[serde(default)]
    bases: Vec<HeapId>,
    #[serde(default)]
    all_bases: Vec<ClassBase>,
    #[serde(default)]
    mro: Vec<ClassBase>,
    #[serde(default)]
    defined_methods: Vec<FunctionId>,
    /// Boundary identity, generated lazily the first time the class (or one of
    /// its instances) crosses to the host; dumped with the heap so it stays
    /// stable across restores.
    uuid: Option<MontyUuid>,
}

impl Class {
    /// Returns the direct user-class base IDs (used for GC and heap walking).
    pub(crate) fn bases(&self) -> &[HeapId] {
        &self.bases
    }

    /// Returns all direct base classes in declaration order (including built-in exceptions and object).
    pub(crate) fn all_bases(&self) -> &[ClassBase] {
        &self.all_bases
    }

    /// The base exception type if this class derives from `BaseException` / `Exception`.
    pub(crate) fn base_exception(&self) -> Option<ExcType> {
        self.mro.iter().find_map(|b| match b {
            ClassBase::BuiltinExc(e) => Some(*e),
            _ => None,
        })
    }

    /// Returns the method resolution order (base classes in C3 linearization order).
    pub(crate) fn mro(&self) -> &[ClassBase] {
        &self.mro
    }

    /// Returns the method IDs defined directly in this class body.
    pub(crate) fn defined_methods(&self) -> &[FunctionId] {
        &self.defined_methods
    }

    /// Whether this class inherits from `target`.
    pub(crate) fn is_subclass_of(&self, target: HeapId) -> bool {
        self.mro.contains(&ClassBase::Class(target))
    }

    /// Searches the class namespace and base classes in C3 MRO order, returning a borrowed reference.
    pub(crate) fn mro_getattr_ref<'a>(
        &'a self,
        attr_str: &str,
        heap: &'a Heap,
        interns: &Interns,
    ) -> Option<&'a Value> {
        // 1. Check own namespace
        if let Some(value) = self.namespace.get_by_str(attr_str, heap, interns) {
            return Some(value);
        }

        // 2. Check base classes in C3 MRO order
        for base in &self.mro {
            if let ClassBase::Class(base_id) = base
                && let HeapData::Class(base_class) = heap.get(*base_id)
                && let Some(val) = base_class.namespace.get_by_str(attr_str, heap, interns)
            {
                return Some(val);
            }
        }

        None
    }

    /// Searches the class namespace and base classes in C3 MRO order, cloning the result.
    pub(crate) fn mro_getattr(&self, attr_str: &str, heap: &Heap, interns: &Interns) -> Option<Value> {
        self.mro_getattr_ref(attr_str, heap, interns)
            .map(|v| v.clone_with_heap(heap))
    }

    /// Creates a new class object from its name, member namespace, direct bases, and C3 MRO.
    #[must_use]
    pub fn new(
        name: EitherStr,
        namespace: Dict,
        bases: Vec<HeapId>,
        all_bases: Vec<ClassBase>,
        mro: Vec<ClassBase>,
        defined_methods: Vec<FunctionId>,
    ) -> Self {
        Self {
            name,
            namespace,
            bases,
            all_bases,
            mro,
            defined_methods,
            uuid: None,
        }
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
        if let Some(name_str) = name.as_either_str(vm.heap)
            && matches!(name_str.as_str(vm.interns), "__class__" | "__bases__")
        {
            [name, value].drop_with(vm);
            return Err(ExcType::type_error("type controls __class__ and __bases__"));
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

pub(crate) fn class_bases_values(all_bases: &[ClassBase], heap: &mut Heap) -> Vec<Value> {
    let mut items = Vec::with_capacity(all_bases.len().max(1));
    for &base in all_bases {
        match base {
            ClassBase::Class(id) => {
                heap.inc_ref(id);
                items.push(Value::Ref(id));
            }
            ClassBase::BuiltinExc(exc) => {
                items.push(Value::Builtin(Builtins::ExcType(exc)));
            }
            ClassBase::Object => {
                items.push(Value::Builtin(Builtins::Type(Type::Object)));
            }
        }
    }
    if items.is_empty() {
        items.push(Value::Builtin(Builtins::Type(Type::Object)));
    }
    items
}

pub(crate) fn class_mro_values(class_id: HeapId, mro: &[ClassBase], heap: &mut Heap) -> Vec<Value> {
    let mut items = Vec::with_capacity(1 + mro.len() + 1);
    heap.inc_ref(class_id);
    items.push(Value::Ref(class_id));
    for &base in mro {
        match base {
            ClassBase::Class(id) => {
                heap.inc_ref(id);
                items.push(Value::Ref(id));
            }
            ClassBase::BuiltinExc(exc) => {
                items.push(Value::Builtin(Builtins::ExcType(exc)));
            }
            ClassBase::Object => {
                items.push(Value::Builtin(Builtins::Type(Type::Object)));
            }
        }
    }
    if !items
        .iter()
        .any(|v| matches!(v, Value::Builtin(Builtins::Type(Type::Object))))
    {
        items.push(Value::Builtin(Builtins::Type(Type::Object)));
    }
    items
}

pub(crate) fn validate_init_subclass_args(target_name: &str, args: ArgValues, heap: &mut Heap) -> RunResult<()> {
    let (pos, kwargs) = args.into_parts();
    let pos_len = pos.as_slice().len();
    pos.drop_with(heap);
    let has_kwargs = !kwargs.is_empty();
    kwargs.drop_with(heap);
    if pos_len > 0 {
        return Err(ExcType::type_error(format!(
            "{target_name}.__init_subclass__() takes no arguments ({pos_len} given)"
        )));
    }
    if has_kwargs {
        return Err(ExcType::type_error(format!(
            "{target_name}.__init_subclass__() takes no keyword arguments"
        )));
    }
    Ok(())
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
        let mut value_guard = DropGuard::new(value, vm);
        let name = attribute_name_value(name, value_guard.ctx());
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

        if attr_str == "__class__" {
            return Ok(Some(CallResult::Value(Value::Builtin(Builtins::Function(
                BuiltinsFunctions::Type,
            )))));
        }

        if attr_str == "__bases__" {
            let all_bases = self.get(vm.heap).all_bases().to_vec();
            let items = class_bases_values(&all_bases, vm.heap);
            let tuple_val = allocate_tuple(items.into(), vm.heap);
            return Ok(Some(CallResult::Value(tuple_val)));
        }

        if attr_str == "__mro__" {
            let class_id = self.id();
            let mro = self.get(vm.heap).mro().to_vec();
            let items = class_mro_values(class_id, &mro, vm.heap);
            let tuple_val = allocate_tuple(items.into(), vm.heap);
            return Ok(Some(CallResult::Value(tuple_val)));
        }

        // Look up a member in the class namespace or its base classes (MRO).
        if let Some(val) = self.get(vm.heap).mro_getattr(attr_str, vm.heap, vm.interns) {
            return Ok(Some(CallResult::Value(val)));
        }

        Err(ExcType::attribute_error_type(
            self.get(vm.heap).name.as_str(vm.interns),
            attr_str,
        ))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        // `__name__` is a synthesized string, not a namespace member (see
        // `py_getattr`), so calling it goes through the normal callable
        // dispatch and raises CPython's `TypeError: 'str' object is not
        // callable` rather than a spurious `AttributeError`.
        if attr_str == "__name__" {
            let name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
            let name_val = allocate_string(name, vm.heap);
            defer_drop!(name_val, vm);
            return vm.call_function(name_val, args);
        }

        // `Foo.method(args)` calls the member with the given args.
        // For `__init_subclass__`, which is an implicit classmethod in Python,
        // we prepend the class itself (self.id()) as the first argument.
        if let Some(member) = self.get(vm.heap).mro_getattr(attr_str, vm.heap, vm.interns) {
            defer_drop!(member, vm);
            let call_args = if attr_str == "__init_subclass__" {
                let self_id = self.id();
                vm.heap.inc_ref(self_id);
                args.prepend(Value::Ref(self_id))
            } else {
                args
            };
            return vm.call_function(member, call_args);
        }

        if attr_str == "mro" {
            let class_name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
            let pos = args.into_pos_only(&format!("{class_name}.mro"), vm.heap)?;
            if !pos.as_slice().is_empty() {
                let count = pos.as_slice().len();
                pos.drop_with(vm.heap);
                return Err(ExcType::type_error(format!(
                    "{class_name}.mro() takes no arguments ({count} given)"
                )));
            }
            let class_id = self.id();
            let mro = self.get(vm.heap).mro().to_vec();
            let items = class_mro_values(class_id, &mro, vm.heap);
            let list_id = vm.heap.allocate(HeapData::List(List::new(items)));
            return Ok(CallResult::Value(Value::Ref(list_id)));
        }

        if attr_str == "__init_subclass__" {
            let class_name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
            validate_init_subclass_args(&class_name, args, vm.heap)?;
            return Ok(CallResult::Value(Value::None));
        }

        args.drop_with(vm);
        Err(ExcType::attribute_error_type(
            self.get(vm.heap).name.as_str(vm.interns),
            attr_str,
        ))
    }
}

impl HeapItem for Class {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.namespace.py_dec_ref_ids(stack);
        for &base_id in &self.bases {
            stack.push(base_id);
        }
    }
}
