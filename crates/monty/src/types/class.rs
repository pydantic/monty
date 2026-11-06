use std::fmt::Write;

use monty_types::MontyUuid;
use smallvec::smallvec;

use super::{
    Dict, LazyHeapSet, OwnedTypeRef, PyTrait, Type, TypeRef,
    instance::{BoundMethod, is_method_value},
    tuple::allocate_tuple,
};
use crate::{
    annotations,
    args::ArgValues,
    boundary_uuid::create_uuid,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadMut,
        HeapReader, heap_read_ref_as_field_mut,
    },
    intern::{Interns, StringId},
    types::{Union, str::allocate_string, r#type::bind_init_subclass},
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
    /// Direct bases in declaration order (`__bases__`), owning their class
    /// references. Empty for a class written before inheritance, read as `(object,)`.
    #[serde(default)]
    bases: Box<[OwnedTypeRef]>,
    /// The C3 linearisation after this class (`__mro__[1:]`), ending at `object`.
    /// Borrowed: each base owns its own ancestors and `__bases__` never changes.
    #[serde(default)]
    mro: Box<[TypeRef]>,
    /// Boundary identity, generated lazily the first time the class (or one of
    /// its instances) crosses to the host; dumped with the heap so it stays
    /// stable across restores.
    uuid: Option<MontyUuid>,
}

impl Class {
    /// Creates a new class object with no explicit bases, so `object` is its only ancestor.
    #[must_use]
    pub fn new(name: EitherStr, namespace: Dict) -> Self {
        Self::with_bases(name, namespace, Box::default(), Box::default())
    }

    /// Creates a class with `bases` (whose class references it takes) and the
    /// C3 linearisation `mro` of those bases, which must end at `object`.
    #[must_use]
    pub(crate) fn with_bases(
        name: EitherStr,
        namespace: Dict,
        bases: Box<[OwnedTypeRef]>,
        mro: Box<[TypeRef]>,
    ) -> Self {
        Self {
            name,
            namespace,
            bases,
            mro,
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

    /// The direct bases (`__bases__`); empty means `(object,)`.
    #[must_use]
    pub(crate) fn bases(&self) -> &[OwnedTypeRef] {
        &self.bases
    }

    /// Looks `name` up in this class's namespace, then in each ancestor class's
    /// in MRO order. Native ancestors carry no namespace, so they are skipped.
    #[must_use]
    pub(crate) fn lookup<'h>(&'h self, name: &str, heap: &'h Heap, interns: &Interns) -> Option<&'h Value> {
        self.namespace.get_by_str(name, heap, interns).or_else(|| {
            self.mro.iter().find_map(|ancestor| match ancestor {
                TypeRef::Class(id) => match heap.get(*id) {
                    HeapData::Class(class) => class.namespace.get_by_str(name, heap, interns),
                    _ => None,
                },
                TypeRef::Native(_) => None,
            })
        })
    }

    /// The ancestors after this class in method resolution order, ending at `object`.
    #[must_use]
    pub(crate) fn ancestors(&self) -> &[TypeRef] {
        /// The MRO tail of a class with no explicit bases.
        const OBJECT_ONLY: &[TypeRef] = &[TypeRef::Native(Type::Object)];
        if self.mro.is_empty() { OBJECT_ONLY } else { &self.mro }
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
        let guarded = name
            .as_either_str(vm.heap)
            .map(|name| name.as_str(vm.interns).to_owned());
        match guarded.as_deref() {
            Some("__bases__" | "__class__") => {
                [name, value].drop_with(vm);
                Err(ExcType::type_error("type controls __class__ and __bases__"))
            }
            Some("__mro__") => {
                [name, value].drop_with(vm);
                Err(ExcType::attribute_error_not_writable("__mro__", "type"))
            }
            _ => self.namespace_mut().set(name, value, vm),
        }
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

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Class> {
    /// Constructing an instance, which runs `__init__` as an ordinary frame.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        vm.instantiate_class(self.id(), args)
    }

    fn py_type(&self, _vm: &VM<'h>) -> TypeRef {
        // The type of a class object is `type` (matching `type(Foo) is type`).
        Type::Type.into()
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
        // PEP 649: computed from the class's own `__annotate__` on first access,
        // which may run Python code, so it cannot go through `class_getattr`.
        if attr.as_str(vm.interns) == "__annotations__" {
            return annotations::class_annotations(self.id(), vm).map(Some);
        }
        match class_getattr(self.id(), attr, vm)? {
            Some(value) => Ok(Some(CallResult::Value(value))),
            None => Err(ExcType::attribute_error_type(
                self.get(vm.heap).name.as_str(vm.interns),
                attr.as_str(vm.interns),
            )),
        }
    }

    /// `Foo.method(args)` calls the raw member with the given args: no `self`
    /// is inserted, the caller passes the instance explicitly. `__init_subclass__`
    /// is an implicit classmethod, which `class_getattr` binds to the class.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let class_id = self.id();
        if attr.as_str(vm.interns) == "__annotations__" {
            let annotations =
                annotations::class_annotations(class_id, vm).and_then(|call| call.require_synchronous(vm));
            let annotations = match annotations {
                Ok(annotations) => annotations,
                Err(err) => {
                    args.drop_with(vm);
                    return Err(err);
                }
            };
            defer_drop!(annotations, vm);
            return vm.call_function(annotations, args);
        }
        let member = match class_getattr(class_id, attr, vm) {
            Ok(member) => member,
            Err(err) => {
                args.drop_with(vm);
                return Err(err);
            }
        };
        let Some(member) = member else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error_type(
                self.get(vm.heap).name.as_str(vm.interns),
                attr.as_str(vm.interns),
            ));
        };
        defer_drop!(member, vm);
        vm.call_function(member, args)
    }
}

/// Reads an attribute of the class `class_id`, `None` when it has no such member.
///
/// Follows CPython's `type_getattro`: the metatype's data descriptors
/// (`__name__`, `__bases__`, `__mro__`) shadow the class's
/// own members, which come next in MRO order, then the members native
/// ancestors provide (`object.__init__`), then `type`'s methods (`mro`).
pub(crate) fn class_getattr(class_id: HeapId, attr: &EitherStr, vm: &mut VM<'_>) -> RunResult<Option<Value>> {
    let HeapData::Class(class) = vm.heap.get(class_id) else {
        unreachable!("class_getattr takes a class")
    };
    let attr_str = attr.as_str(vm.interns);
    Ok(Some(match attr_str {
        "__name__" => allocate_string(class.name.as_str(vm.interns).to_owned(), vm.heap),
        "__bases__" => bases_tuple(class, vm.heap),
        "__mro__" => mro_tuple(class_id, vm.heap),
        // Annotations are per class: an inherited `__annotate__` would describe the base.
        "__annotate__" | "__annotate_func__" => class
            .namespace
            .get_by_str(attr_str, vm.heap, vm.interns)
            .map_or(Value::None, |value| value.clone_with_heap(vm.heap)),
        _ => {
            if let Some(member) = class.lookup(attr_str, vm.heap, vm.interns) {
                let member = member.clone_with_heap(vm.heap);
                if attr_str == "__init_subclass__" && is_method_value(&member, vm) {
                    vm.heap.inc_ref(class_id);
                    bind_init_subclass(Value::Ref(class_id), member, vm.heap)
                } else {
                    member
                }
            } else if let Some(native) = class.ancestors().iter().find_map(|ancestor| ancestor.native())
                && let Some(member) = native.class_constant(attr, vm)
            {
                if attr_str == "__init_subclass__" {
                    vm.heap.inc_ref(class_id);
                    bind_init_subclass(Value::Ref(class_id), member, vm.heap)
                } else {
                    member
                }
            } else if attr_str == "mro" {
                vm.heap.inc_ref(class_id);
                BoundMethod::allocate(
                    Value::Ref(class_id),
                    Value::Builtin(Builtins::Function(BuiltinsFunctions::TypeMro)),
                    vm.heap,
                )
            } else if attr_str == "__class__" {
                Value::Builtin(Builtins::Type(Type::Type))
            } else {
                return Ok(None);
            }
        }
    }))
}

/// `cls.__bases__`: the declared bases, or `(object,)` for a class with none.
fn bases_tuple(class: &Class, heap: &Heap) -> Value {
    let items = if class.bases.is_empty() {
        smallvec![Value::Builtin(Builtins::Type(Type::Object))]
    } else {
        class.bases.iter().map(|base| base.borrow().to_value(heap)).collect()
    };
    allocate_tuple(items, heap)
}

/// `cls.__mro__`: the class itself followed by its ancestors.
pub(crate) fn mro_tuple(class_id: HeapId, heap: &Heap) -> Value {
    let items = TypeRef::Class(class_id).mro(heap).map(|ty| ty.to_value(heap)).collect();
    allocate_tuple(items, heap)
}

impl HeapItem for Class {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        stack.extend(self.bases.iter().filter_map(|base| base.class_id()));
        self.namespace.py_dec_ref_ids(stack);
    }
}
