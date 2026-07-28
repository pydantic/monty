use std::{fmt::Write, mem};

use super::{Dict, LazyHeapSet, PyTrait, Type};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{
        BorrowedHeapReadMut, ContainsHeap, DropWithContext, HeapId, HeapItem, HeapRead, heap_read_ref_as_field_mut,
    },
    intern::StringId,
    types::str::allocate_string,
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
    /// Present when `@dataclass` has been applied — drives native synthesis of
    /// `__init__`/`__repr__`/`__eq__`/etc. off the field metadata. `None` for an
    /// ordinary class. `#[serde(default)]` so pre-dataclass snapshots still load.
    #[serde(default)]
    dataclass_meta: Option<DataclassMeta>,
}

/// Metadata recorded on a [`Class`] by the `@dataclass` decorator.
///
/// **Holds owned heap references** through its captured defaults, so a `Class`
/// carrying one counts them in `py_estimate_size`, `py_dec_ref_ids` (both below)
/// and `heap::for_each_child_id`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassMeta {
    /// Fields in definition order (from `__annotations__`, `ClassVar` excluded).
    pub fields: Vec<DataclassField>,
}

impl DataclassMeta {
    /// Bytes this metadata adds to its `Class`: the field vector's allocation
    /// alone, since the struct itself is inline in `size_of::<Class>()`.
    ///
    /// Decoration grows an already-allocated `Class` in place, so the caller must
    /// charge this to the tracker — otherwise a sandbox could decorate wide
    /// classes past `max_memory`.
    #[must_use]
    pub fn estimate_size(&self) -> usize {
        self.fields.capacity() * mem::size_of::<DataclassField>()
    }

    /// Every captured default that is a heap reference: the metadata's children.
    pub fn ref_children(&self) -> impl Iterator<Item = HeapId> + '_ {
        self.fields.iter().filter_map(|f| match &f.default {
            Some(Value::Ref(id)) => Some(*id),
            _ => None,
        })
    }
}

/// Releases the captured defaults, for metadata being discarded rather than
/// stored on a `Class` — a rejected decoration, or the metadata a re-decoration
/// replaced. Lets the decorator guard its work-in-progress metadata like any
/// other owned value.
impl<C: ContainsHeap> DropWithContext<C> for DataclassMeta {
    fn drop_with(self, ctx: &mut C) {
        for field in self.fields {
            field.default.drop_with(ctx);
        }
    }
}

/// A single dataclass field's metadata (see [`DataclassMeta`]).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DataclassField {
    /// The interned field name (from an `__annotations__` key, always interned).
    pub name: StringId,
    /// The default **captured when `@dataclass` ran** (an owned reference), or
    /// `None` for a required field. CPython bakes defaults into the generated
    /// `__init__`, so rebinding the class attribute later must not change it.
    pub default: Option<Value>,
    /// Whether the annotation was `InitVar[...]`. Recorded but not acted on —
    /// decoration rejects such a field until pseudo-fields are implemented.
    #[serde(default)]
    pub initvar: bool,
}

impl Class {
    /// Creates a new class object from its name and member namespace. Ordinary
    /// classes start with no dataclass metadata; `@dataclass` sets it later.
    #[must_use]
    pub fn new(name: EitherStr, namespace: Dict) -> Self {
        Self {
            name,
            namespace,
            dataclass_meta: None,
        }
    }

    /// Returns the dataclass metadata, or `None` for an ordinary class.
    #[must_use]
    pub fn dataclass_meta(&self) -> Option<&DataclassMeta> {
        self.dataclass_meta.as_ref()
    }

    /// Records dataclass metadata (called by the `@dataclass` decorator), after
    /// the caller has charged [`DataclassMeta::estimate_size`] to the tracker.
    ///
    /// Re-decorating returns the replaced metadata, whose captured defaults the
    /// caller **must** drop and whose bytes it must refund — nothing else does.
    #[must_use]
    pub fn set_dataclass_meta(&mut self, meta: DataclassMeta) -> Option<DataclassMeta> {
        self.dataclass_meta.replace(meta)
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
        self.namespace_mut().set(name, value, vm)
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Class> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        // The type of a class object is `type` (matching `type(Foo) is type`).
        Type::Type
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _other: &Value, _vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        // Classes compare by identity, which `Value::py_eq_impl` resolves before
        // ever reaching here; from this side every class is `NotImplemented`.
        Ok(None)
    }

    fn py_hash(&self, self_id: HeapId, _vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        // Class objects hash by identity (like CPython type objects).
        Ok(Some(identity_hash(self_id)))
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
            return Ok(Some(CallResult::Value(allocate_string(name, vm.heap)?)));
        }
        // Otherwise look up a member (method or class variable) in the namespace.
        match self.get(vm.heap).namespace.get_by_str(attr_str, vm.heap, vm.interns) {
            Some(value) => Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap)))),
            None => Err(ExcType::attribute_error_type(
                self.get(vm.heap).name.as_str(vm.interns),
                attr_str,
            )),
        }
    }

    fn py_call_attr(
        &mut self,
        _self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        // `__name__` is a synthesized string, not a namespace member (see
        // `py_getattr`), so calling it goes through the normal callable
        // dispatch and raises CPython's `TypeError: 'str' object is not
        // callable` rather than a spurious `AttributeError`.
        if attr_str == "__name__" {
            let name = self.get(vm.heap).name.as_str(vm.interns).to_owned();
            let name_val = match allocate_string(name, vm.heap) {
                Ok(v) => v,
                Err(e) => {
                    args.drop_with(vm);
                    return Err(e.into());
                }
            };
            defer_drop!(name_val, vm);
            return vm.call_function(name_val, args);
        }
        // `Foo.method(args)` calls the raw (unbound) member with the given args —
        // no `self` is inserted, the caller passes the instance explicitly.
        let member = self
            .get(vm.heap)
            .namespace
            .get_by_str(attr_str, vm.heap, vm.interns)
            .map(|v| v.clone_with_heap(vm.heap));
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
    fn py_estimate_size(&self) -> usize {
        mem::size_of::<Self>()
            + self.name.py_estimate_size()
            + self.namespace.py_estimate_size()
            + self.dataclass_meta.as_ref().map_or(0, DataclassMeta::estimate_size)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.namespace.py_dec_ref_ids(stack);
        // Captured dataclass defaults are owned references too.
        if let Some(meta) = &mut self.dataclass_meta {
            for field in &mut meta.fields {
                if let Some(default) = &mut field.default {
                    default.py_dec_ref_ids(stack);
                }
            }
        }
    }
}
