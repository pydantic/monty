use std::fmt::Write;

use monty_types::MontyUuid;

use super::{Dict, LazyHeapSet, PyTrait, Type, attribute_name_value};
use crate::{
    args::ArgValues,
    boundary_uuid::create_uuid,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead,
        heap_read_ref_as_field_mut,
    },
    types::{Union, str::allocate_string},
    value::{EitherStr, Value},
};

/// The `@dataclass(...)` options Monty implements.
///
/// Small and `Copy`, so it doubles as the payload of the *configured decorator*
/// (`dataclass(frozen=True)`) without a heap allocation. Every other CPython
/// flag is rejected at the call, so each is either stored here or known to hold
/// its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassOptions {
    /// Synthesize a field-wise `__eq__` (CPython's `eq`, default `True`).
    pub eq: bool,
    /// Reject attribute assignment, and hash by field values when `eq` is also
    /// set (CPython's `frozen`, default `False`).
    pub frozen: bool,
}

impl Default for DataclassOptions {
    /// CPython's defaults: `eq=True, frozen=False`.
    fn default() -> Self {
        Self {
            eq: true,
            frozen: false,
        }
    }
}

/// A user-defined class object created by a `class Foo: ...` statement.
///
/// Holds the class name and a `namespace` [`Dict`] mapping member names to values:
/// methods (stored as `DefFunction`/`Closure` values) and class variables. The
/// class's own [`HeapId`] is its type identity — `type(x) is Foo` and `isinstance`
/// work via reference identity, so there is no separate type-id counter.
///
/// Calling a class (`Foo(...)`) constructs an [`Instance`](super::Instance); see
/// `instantiate_class` in the VM's call module. Member lookup walks the parent chain.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Class {
    /// Class name (e.g. `Foo`), used for `repr` and `__name__`. Interned for
    /// compiled `class` statements; heap-owned for classes created at runtime
    /// via the 3-arg `type(name, bases, dict)` form, whose name cannot be
    /// interned because the intern table is frozen after prepare.
    name: EitherStr,
    /// Members: method name / class-variable name -> value.
    namespace: Dict,
    /// Direct bases; sandbox class IDs are owned references.
    #[serde(default)]
    bases: Vec<Type>,
    /// The `@dataclass(...)` options this class was decorated with, left at
    /// CPython's defaults for a class that was not. Stands in for the dunders
    /// CPython generates and Monty cannot yet install: baked in at decoration
    /// so `__dataclass_params__` stays a report, not a rewritable control.
    options: DataclassOptions,
    /// Boundary identity, generated lazily the first time the class (or one of
    /// its instances) crosses to the host; dumped with the heap so it stays
    /// stable across restores.
    uuid: Option<MontyUuid>,
}

impl Class {
    /// Takes ownership of the namespace and base references.
    ///
    /// Dataclass options start at their defaults; `@dataclass` sets them with
    /// [`HeapRead::set_dataclass_options`] once it has built the class.
    #[must_use]
    pub fn new(name: EitherStr, namespace: Dict, bases: Vec<Type>) -> Self {
        Self {
            name,
            namespace,
            bases,
            options: DataclassOptions::default(),
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

    /// The `@dataclass(...)` options in force for this class.
    ///
    /// Meaningful only once [`dataclass_options`](crate::modules::dataclasses::dataclass_options)
    /// has confirmed the class is a dataclass — a plain class reports the
    /// defaults it was never decorated with.
    #[must_use]
    pub fn dataclass_options(&self) -> DataclassOptions {
        self.options
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

    pub fn bases(&self) -> &[Type] {
        &self.bases
    }

    fn parent(&self) -> Option<HeapId> {
        match self.bases.first()? {
            Type::Instance(id) => Some(*id),
            _ => None,
        }
    }
}

/// Searches the class and its ancestors, stopping at the first matching member.
pub(crate) fn lookup_member(class_id: HeapId, name: &str, vm: &VM<'_>) -> Option<Value> {
    lookup_member_ref(class_id, name, vm)
        .map(|value| value.clone_with_heap(vm.heap))
        .or_else(|| super::native_class::member(native_base(class_id, vm).unwrap_or(Type::Object), name))
}

/// Borrows the first matching member without acquiring a heap reference.
pub(crate) fn lookup_member_ref<'v>(mut class_id: HeapId, name: &str, vm: &'v VM<'_>) -> Option<&'v Value> {
    loop {
        let HeapData::Class(class) = vm.heap.get(class_id) else {
            return None;
        };
        if let Some(value) = class.namespace().get_by_str(name, vm.heap, vm.interns) {
            return Some(value);
        }
        class_id = class.parent()?;
    }
}

/// Whether a sandbox class is the parent itself or one of its descendants.
pub(crate) fn class_is_subclass(mut class_id: HeapId, parent: HeapId, vm: &VM<'_>) -> bool {
    loop {
        if class_id == parent {
            return true;
        }
        let HeapData::Class(class) = vm.heap.get(class_id) else {
            return false;
        };
        let Some(base) = class.parent() else {
            return false;
        };
        class_id = base;
    }
}

pub(crate) fn native_base(mut class: HeapId, vm: &VM<'_>) -> Option<Type> {
    loop {
        let HeapData::Class(data) = vm.heap.get(class) else {
            return None;
        };
        match data.bases().first()? {
            Type::Instance(parent) => class = *parent,
            native => return Some(*native),
        }
    }
}

impl<'h> HeapRead<'h, Class> {
    fn namespace_mut(&mut self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, Class, namespace)
    }

    /// Sets a class attribute (`Foo.x = 1`), returning the previous value (if any)
    /// for the caller to drop. Takes ownership of both `name` and `value`.
    ///
    /// Existing instances observe the change immediately: instance attribute reads
    /// fall through to this namespace.
    pub fn set_attr(&mut self, name: Value, value: Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if let Some(name_str) = name.as_either_str(vm.heap)
            && matches!(name_str.as_str(vm.interns), "__class__" | "__bases__")
        {
            [name, value].drop_with(vm);
            return Err(ExcType::type_error("type controls __class__ and __bases__"));
        }
        self.namespace_mut().set(name, value, vm)
    }

    /// Records what `@dataclass(...)` decorated this class with.
    ///
    /// Called once per decoration, so re-decorating replaces the options as it
    /// replaces the fields. Assigning to `__dataclass_params__` afterwards does
    /// not reach here, which is what makes that object a report rather than a
    /// control.
    pub fn set_dataclass_options(&mut self, options: DataclassOptions, vm: &mut VM<'h>) {
        self.get_mut(vm.heap).options = options;
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
        if attr_str == "__class__" {
            return Ok(Some(CallResult::Value(Value::Builtin(Builtins::Function(
                BuiltinsFunctions::Type,
            )))));
        }
        if attr_str == "__bases__" {
            let values = self
                .get(vm.heap)
                .bases()
                .iter()
                .map(|base| match base {
                    Type::Instance(id) => {
                        vm.heap.inc_ref(*id);
                        Value::Ref(*id)
                    }
                    Type::Exception(exc) => Value::Builtin(Builtins::ExcType(*exc)),
                    other => Value::Builtin(Builtins::Type(*other)),
                })
                .collect();
            return Ok(Some(CallResult::Value(super::allocate_tuple(values, vm.heap))));
        }
        if attr_str == "__name__" {
            let name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
            return Ok(Some(CallResult::Value(allocate_string(name, vm.heap))));
        }
        match lookup_member(self.id(), attr_str, vm) {
            Some(value) => Ok(Some(CallResult::Value(value))),
            None => Err(ExcType::attribute_error_type(
                self.get(vm.heap).name.as_str(vm.interns),
                attr_str,
            )),
        }
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
        // `Foo.method(args)` calls the raw (unbound) member with the given args —
        // no `self` is inserted, the caller passes the instance explicitly.
        let member = lookup_member(self.id(), attr_str, vm);
        if let Some(member) = member {
            defer_drop!(member, vm);
            vm.call_function(member, args)
        } else {
            args.drop_with(vm);
            Err(ExcType::attribute_error_type(
                self.get(vm.heap).name.as_str(vm.interns),
                attr_str,
            ))
        }
    }
}

impl HeapItem for Class {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for base in &self.bases {
            if let Type::Instance(id) = base {
                stack.push(*id);
            }
        }
        self.namespace.py_dec_ref_ids(stack);
    }
}
