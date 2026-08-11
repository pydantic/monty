use std::{
    fmt::Write,
    hash::{DefaultHasher, Hash, Hasher},
};

use serde::ser::SerializeStruct;

use super::{Dict, LazyHeapSet, PyTrait, attribute_name_value};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    hash::HashValue,
    heap::{
        BorrowedHeapRead, BorrowedHeapReadMut, DropGuard, DropWithContext, HeapId, HeapItem, HeapRead, HeapReadOutput,
        heap_read_ref_as_field, heap_read_ref_as_field_mut,
    },
    intern::Interns,
    types::Type,
    value::{EitherStr, Value},
};

/// Python dataclass instance type.
///
/// Represents an instance of a dataclass with a class name, field values, and
/// frozen/mutable semantics. Method calls on dataclasses are detected lazily:
/// when `call_attr` is invoked on a dataclass and the attribute name is not found
/// in `attrs`, it is dispatched as a `MethodCall` to the host (provided the name
/// is public — no leading underscore).
///
/// # Fields
/// - `name`: The class name (e.g., "Point", "User")
/// - `field_names`: Declared field names in definition order (used for repr)
/// - `attrs`: All attributes including declared fields and dynamically added ones
/// - `frozen`: Whether the dataclass instance is immutable
///
/// # Hashability
/// When `frozen` is true, the dataclass is immutable and hashable. The hash
/// is computed from the class name and declared field values only.
/// When `frozen` is false, the dataclass is mutable and unhashable.
///
/// # Reference Counting
/// The `attrs` Dict contains Values that may be heap-allocated. The
/// `py_dec_ref_ids` method properly handles decrementing refcounts for
/// all attribute values when the dataclass instance is freed.
///
/// # Attribute Access
/// - Getting: Looks up the attribute name in the attrs Dict
/// - Setting: Updates or adds the attribute in attrs (only if not frozen)
/// - Method calls: If the attribute is a public name not found in attrs, dispatched to host
/// - repr: Only shows declared fields (from field_names), not extra attributes
#[derive(Debug)]
pub(crate) struct Dataclass {
    /// The class name (e.g., "Point", "User")
    name: EitherStr,
    /// Identifier of the type, from `id(type(dc))` in python.
    type_id: u64,
    /// Declared field names in definition order (for repr and hashing)
    field_names: Vec<String>,
    /// All attributes (both declared fields and dynamically added)
    attrs: Dict,
    /// Whether this dataclass instance is immutable (affects hashability)
    frozen: bool,
}

impl Dataclass {
    /// Creates a new dataclass instance.
    ///
    /// # Arguments
    /// * `name` - The class name
    /// * `type_id` - The type ID of the dataclass
    /// * `field_names` - Declared field names in definition order
    /// * `attrs` - Dict of attribute name -> value pairs (ownership transferred)
    /// * `frozen` - Whether this dataclass instance is immutable (affects hashability)
    #[must_use]
    pub fn new(name: impl Into<EitherStr>, type_id: u64, field_names: Vec<String>, attrs: Dict, frozen: bool) -> Self {
        Self {
            name: name.into(),
            type_id,
            field_names,
            attrs,
            frozen,
        }
    }

    /// Returns the class name.
    #[must_use]
    pub fn name<'a>(&'a self, interns: &'a Interns) -> &'a str {
        self.name.as_str(interns)
    }

    /// Returns the type ID of the dataclass.
    #[must_use]
    pub fn type_id(&self) -> u64 {
        self.type_id
    }

    /// Returns a reference to the declared field names.
    #[must_use]
    pub fn field_names(&self) -> &[String] {
        &self.field_names
    }

    /// Returns a reference to the attrs Dict.
    #[must_use]
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }

    /// Returns whether this dataclass instance is frozen (immutable).
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }
}

impl<'h> HeapRead<'h, Dataclass> {
    /// Sets an attribute value.
    ///
    /// The caller transfers ownership of both `name` and `value`. Returns the
    /// old value if the attribute existed (caller must drop it), or None if this
    /// is a new attribute.
    ///
    /// Returns `FrozenInstanceError` if the dataclass is frozen.
    pub fn set_attr(&mut self, name: Value, value: Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if self.get(vm.heap).frozen {
            defer_drop!(name, vm);
            value.drop_with(vm);
            let name_repr = name.py_repr(vm)?;
            defer_drop!(name_repr, vm);
            let exc = SimpleException::new_msg(
                ExcType::FrozenInstanceError,
                format!("cannot assign to field {}", name_repr.to_str(vm)?),
            );
            return Err(exc.into());
        }
        self.attrs_mut().set(name, value, vm)
    }

    pub fn attrs(&self) -> BorrowedHeapRead<'_, 'h, Dict> {
        heap_read_ref_as_field!(self, Dataclass, attrs)
    }

    pub fn attrs_mut(&mut self) -> BorrowedHeapReadMut<'_, 'h, Dict> {
        heap_read_ref_as_field_mut!(self, Dataclass, attrs)
    }
}

impl<'h> PyTrait<'h> for HeapRead<'h, Dataclass> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Dataclass
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        // Dataclasses don't have a length
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

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::Dataclass(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        // Dataclasses are equal only if they are the same class and have equal attrs.
        if self.get(vm.heap).type_id() != other.get(vm.heap).type_id() {
            return Ok(Some(false));
        }
        Ok(Some(self.attrs().eq_dict(&other.attrs(), vm)?))
    }

    /// Hashes a frozen dataclass by its class name and the values of declared fields.
    ///
    /// Mutable (non-frozen) dataclasses return `None` (unhashable).
    fn py_hash(&self, _self_id: HeapId, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        // Only frozen (immutable) dataclasses are hashable
        if !self.get(vm.heap).frozen {
            return Ok(None);
        }
        let mut guard = vm.recursion_guard()?;
        let vm = &mut *guard;
        let mut hasher = DefaultHasher::new();
        // Hash the class name
        self.get(vm.heap).name.hash(&mut hasher);
        // Hash each declared field (name, value) pair in order
        let field_count = self.get(vm.heap).field_names.len();
        for i in 0..field_count {
            let field_name = &self.get(vm.heap).field_names[i];
            field_name.hash(&mut hasher);
            if let Some(value) = self.get(vm.heap).attrs.get_by_str(field_name, vm.heap, vm.interns) {
                let value = value.clone_with_heap(vm.heap);
                defer_drop!(value, vm);
                match value.py_hash(vm)? {
                    Some(h) => h.hash(&mut hasher),
                    None => return Ok(None),
                }
            }
        }
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        // Dataclass instances are always truthy (like Python objects)
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        // Only declared fields are shown, not dynamically added attributes.
        let name = self.get(vm.heap).name(vm.interns).to_owned();
        let field_count = self.get(vm.heap).field_names.len();
        write_dataclass_repr(f, &name, field_count, vm, heap_ids, |i, vm| {
            let dc = self.get(vm.heap);
            let field_name = dc.field_names[i].clone();
            let value = dc
                .attrs
                .get_by_str(&field_name, vm.heap, vm.interns)
                .map(|v| v.clone_with_heap(vm.heap));
            Ok((field_name, value))
        })
    }

    /// Performs lazy method detection for dataclass instances.
    ///
    /// If the attribute is a public name (no leading underscore) not found in the
    /// dataclass's attrs dict, returns `MethodCall` so the VM yields to the host.
    /// Otherwise handles the call directly:
    /// - Attributes that exist in attrs but aren't callable produce `TypeError`
    /// - Private/dunder attributes that aren't in attrs produce `AttributeError`
    fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);
        // Only public methods (no underscore prefix = no dunders, no private)
        if !attr_str.starts_with('_')
            && self
                .get(vm.heap)
                .attrs
                .get_by_str(attr_str, vm.heap, vm.interns)
                .is_none()
        {
            // Clone self and prepend to args for the method call
            // inc_ref works even when data is taken out (refcount metadata is separate)
            vm.heap.inc_ref(self_id);
            let self_arg = Value::Ref(self_id);
            let args_with_self = args.prepend(self_arg);
            Ok(CallResult::MethodCall(attr.clone(), args_with_self))
        } else {
            // Not a method call — handle directly
            let method_name = attr.as_str(vm.interns);
            defer_drop!(args, vm);

            // If the attribute exists in attrs, it's a data value (not callable)
            if let Some(value) = self.get(vm.heap).attrs.get_by_str(method_name, vm.heap, vm.interns) {
                let type_name = value.py_type_name(vm);
                Err(ExcType::type_error_not_callable_object(&type_name))
            } else {
                // Attribute doesn't exist — use the class name (e.g., "Point") not "Dataclass"
                Err(ExcType::attribute_error(
                    self.get(vm.heap).name(vm.interns),
                    method_name,
                ))
            }
        }
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr_name = attr.as_str(vm.interns);
        match self.get(vm.heap).attrs.get_by_str(attr_name, vm.heap, vm.interns) {
            Some(value) => Ok(Some(CallResult::Value(value.clone_with_heap(vm.heap)))),
            // we use name here, not `self.py_type(heap)` hence returning a Ok(None)
            None => Err(ExcType::attribute_error(self.get(vm.heap).name(vm.interns), attr_name)),
        }
    }
}

/// Writes `ClassName(f1=v1, ...)`, shared by the host-supplied [`Dataclass`] and
/// native `@dataclass` instances so the two renderings cannot drift.
///
/// Each caller supplies its own field list via `field`, mapping an index to that
/// field's name and a cloned value (dropped here). A cycle renders `...`, a
/// `None` value `<?>`, and exhausting `max_duration` truncates `...[timeout]`.
///
/// `field` is resolved immediately before that field is written, never all up
/// front, so a `__repr__` that mutates a later field is observed — matching the
/// left-to-right evaluation of CPython's generated f-string.
pub(crate) fn write_dataclass_repr<'h>(
    f: &mut impl Write,
    name: &str,
    field_count: usize,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
    field: impl Fn(usize, &mut VM<'h>) -> RunResult<(String, Option<Value>)>,
) -> RunResult<()> {
    let Ok(mut guard) = vm.recursion_guard() else {
        return Ok(f.write_str("...")?);
    };
    let vm = &mut *guard;
    f.write_str(name)?;
    f.write_char('(')?;
    for i in 0..field_count {
        if i > 0 {
            // Same between-item checkpoint as sequence repr, so a wide dataclass
            // cannot outrun `max_duration`.
            if vm.heap.check_time().is_err() {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        // Guarded before anything is written, so a formatter error on the name
        // cannot strand the value the callback just cloned.
        let (field_name, value) = field(i, &mut *vm)?;
        defer_drop!(value, vm);
        f.write_str(&field_name)?;
        f.write_char('=')?;
        match value {
            Some(value) => value.py_repr_fmt(f, vm, heap_ids)?,
            None => f.write_str("<?>")?,
        }
    }
    Ok(f.write_char(')')?)
}

impl HeapItem for Dataclass {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Delegate to the attrs Dict which handles all nested heap references
        self.attrs.py_dec_ref_ids(stack);
    }
}

// Custom serde implementation for Dataclass.
// Serializes all five fields.
impl serde::Serialize for Dataclass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Dataclass", 5)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("type_id", &self.type_id)?;
        state.serialize_field("field_names", &self.field_names)?;
        state.serialize_field("attrs", &self.attrs)?;
        state.serialize_field("frozen", &self.frozen)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Dataclass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct DataclassData {
            name: EitherStr,
            type_id: u64,
            field_names: Vec<String>,
            attrs: Dict,
            frozen: bool,
        }
        let dc = DataclassData::deserialize(deserializer)?;
        Ok(Self {
            name: dc.name,
            type_id: dc.type_id,
            field_names: dc.field_names,
            attrs: dc.attrs,
            frozen: dc.frozen,
        })
    }
}
