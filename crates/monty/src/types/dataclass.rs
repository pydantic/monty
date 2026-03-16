use std::fmt::Write;

use ahash::AHashSet;

use super::{Dict, PyTrait};
use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapId, HeapRead, heap_read_as_field},
    intern::Interns,
    resource::{ResourceError, ResourceTracker},
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

    /// Returns whether this dataclass contains any heap references (`Value::Ref`).
    ///
    /// Delegates to the underlying attrs Dict.
    #[inline]
    #[must_use]
    pub fn has_refs(&self) -> bool {
        self.attrs.has_refs()
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
    /// Returns the class name.
    #[must_use]
    pub fn name<'a>(&'a self, vm: &'a VM<'h, '_, impl ResourceTracker>) -> &'a str {
        self.get(vm.heap).name.as_str(vm.interns)
    }

    /// Sets an attribute value.
    ///
    /// The caller transfers ownership of both `name` and `value`. Returns the
    /// old value if the attribute existed (caller must drop it), or None if this
    /// is a new attribute.
    ///
    /// Returns `FrozenInstanceError` if the dataclass is frozen.
    pub fn set_attr(
        self,
        name: Value,
        value: Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        if self.get(vm.heap).frozen {
            // Get attribute name for error message
            let attr_name = match &name {
                Value::InternString(id) => vm.interns.get_str(*id).to_string(),
                _ => "<unknown>".to_string(),
            };
            // Drop the values we were given ownership of
            name.drop_with_heap(vm);
            value.drop_with_heap(vm);
            return Err(ExcType::frozen_instance_error(&attr_name));
        }
        self.attrs().set(name, value, vm)
    }

    pub fn attrs(self) -> HeapRead<'h, Dict> {
        heap_read_as_field!(self, Dataclass, attrs)
    }
}

impl PyTrait<'_> for Dataclass {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Dataclass
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.py_estimate_size()
            + self.field_names.iter().map(String::len).sum::<usize>()
            + self.attrs.py_estimate_size()
    }

    fn py_len(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> Option<usize> {
        // Dataclasses don't have a length
        None
    }

    fn py_eq(&self, other: &Self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<bool, ResourceError> {
        // Dataclasses are equal if they have the same name and equal attrs
        Ok(self.name == other.name && self.attrs.py_eq(&other.attrs, vm)?)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Delegate to the attrs Dict which handles all nested heap references
        self.attrs.py_dec_ref_ids(stack);
    }

    fn py_bool(&self, _vm: &VM<'_, '_, impl ResourceTracker>) -> bool {
        // Dataclass instances are always truthy (like Python objects)
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        vm: &VM<'_, '_, impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
    ) -> std::fmt::Result {
        // Check depth limit before recursing
        let heap = &*vm.heap;
        let Some(token) = heap.incr_recursion_depth_for_repr() else {
            return f.write_str("...");
        };
        crate::defer_drop_immutable_heap!(token, heap);

        // Format: ClassName(field1=value1, field2=value2, ...)
        // Only declared fields are shown, not dynamically added attributes
        f.write_str(self.name(vm.interns))?;
        f.write_char('(')?;

        let mut first = true;
        for field_name in &self.field_names {
            if !first {
                f.write_str(", ")?;
            }
            first = false;

            // Write field name
            f.write_str(field_name)?;
            f.write_char('=')?;

            // Look up value in attrs
            if let Some(value) = self.attrs.get_by_str(field_name, heap, vm.interns) {
                value.py_repr_fmt(f, vm, heap_ids)?;
            } else {
                // Field not found - shouldn't happen for well-formed dataclasses
                f.write_str("<?>")?;
            }
        }

        f.write_char(')')?;
        Ok(())
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
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let heap = &mut *vm.heap;
        let interns = vm.interns;
        let attr_str = attr.as_str(interns);
        // Only public methods (no underscore prefix = no dunders, no private)
        if !attr_str.starts_with('_') && self.attrs.get_by_str(attr_str, heap, interns).is_none() {
            // Clone self and prepend to args for the method call
            // inc_ref works even when data is taken out (refcount metadata is separate)
            heap.inc_ref(self_id);
            let self_arg = Value::Ref(self_id);
            let args_with_self = args.prepend(self_arg);
            Ok(CallResult::MethodCall(attr.clone(), args_with_self))
        } else {
            // Not a method call — handle directly
            let method_name = attr.as_str(interns);
            defer_drop!(args, heap);

            // If the attribute exists in attrs, it's a data value (not callable)
            if let Some(value) = self.attrs.get_by_str(method_name, heap, interns) {
                let type_name = value.py_type(heap);
                Err(ExcType::type_error_not_callable_object(type_name))
            } else {
                // Attribute doesn't exist — use the class name (e.g., "Point") not "Dataclass"
                Err(ExcType::attribute_error(self.name(interns), method_name))
            }
        }
    }
}

impl<'h> HeapRead<'h, Dataclass> {
    /// Dispatches a method call on a heap-allocated dataclass via the `HeapRead` pattern.
    ///
    /// Public attributes not found in `attrs` are dispatched as `MethodCall` for the
    /// host to resolve. Existing attrs that aren't callable produce `TypeError`.
    pub(crate) fn call_attr(
        self,
        self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let attr_str = attr.as_str(vm.interns);

        // Public methods: attribute not starting with '_' and not in attrs dict
        if !attr_str.starts_with('_') {
            let found_in_attrs = self
                .get(vm.heap)
                .attrs
                .get_by_str(attr_str, vm.heap, vm.interns)
                .is_some();
            if !found_in_attrs {
                // Dispatch as method call — inc ref and prepend self
                vm.heap.inc_ref(self_id);
                let self_arg = Value::Ref(self_id);
                let args_with_self = args.prepend(self_arg);
                return Ok(CallResult::MethodCall(attr.clone(), args_with_self));
            }
        }

        // Not a method call — handle directly
        let method_name = attr.as_str(vm.interns);
        defer_drop!(args, vm);

        // If the attribute exists in attrs, it's a data value (not callable)
        if let Some(value) = self.get(vm.heap).attrs.get_by_str(method_name, vm.heap, vm.interns) {
            let type_name = value.py_type(vm.heap);
            Err(ExcType::type_error_not_callable_object(type_name))
        } else {
            // Use the class name (e.g., "Point") not "Dataclass"
            let name = self.get(vm.heap).name(vm.interns);
            Err(ExcType::attribute_error(name, method_name))
        }
    }
}

// Custom serde implementation for Dataclass.
// Serializes all five fields.
impl serde::Serialize for Dataclass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
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
