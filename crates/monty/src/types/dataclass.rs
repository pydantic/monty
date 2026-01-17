use std::fmt::Write;

use ahash::AHashSet;

use super::{Dict, PyTrait};
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapId},
    intern::Interns,
    resource::ResourceTracker,
    types::Type,
    value::{Attr, Value},
};

/// Python dataclass instance type.
///
/// Represents an instance of a dataclass with a class name, field values, and
/// a set of method names that trigger external function calls when invoked.
///
/// # Fields
/// - `name`: The class name (e.g., "Point", "User")
/// - `field_names`: Declared field names in definition order (used for repr)
/// - `attrs`: All attributes including declared fields and dynamically added ones
/// - `methods`: Set of method names that should trigger external calls
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
/// - Method calls: If the attribute name is in `methods`, triggers external call
/// - repr: Only shows declared fields (from field_names), not extra attributes
#[derive(Debug)]
pub(crate) struct Dataclass {
    /// The class name (e.g., "Point", "User")
    name: String,
    /// Identifier of the type, from `id(type(dc))` in python.
    type_id: u64,
    /// Declared field names in definition order (for repr and hashing)
    field_names: Vec<String>,
    /// All attributes (both declared fields and dynamically added)
    attrs: Dict,
    /// Method names that trigger external function calls
    methods: AHashSet<String>,
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
    /// * `methods` - Set of method names that trigger external calls
    /// * `frozen` - Whether this dataclass instance is immutable (affects hashability)
    #[must_use]
    pub fn new(
        name: String,
        type_id: u64,
        field_names: Vec<String>,
        attrs: Dict,
        methods: AHashSet<String>,
        frozen: bool,
    ) -> Self {
        Self {
            name,
            type_id,
            field_names,
            attrs,
            methods,
            frozen,
        }
    }

    /// Returns the class name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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

    /// Returns a reference to the methods set.
    #[must_use]
    pub fn methods(&self) -> &AHashSet<String> {
        &self.methods
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

    /// Gets an attribute value by name.
    ///
    /// Returns Ok(Some(&Value)) if the attribute exists, Ok(None) if it doesn't.
    /// Returns Err if the key is unhashable (should not happen with string keys).
    pub fn get_attr(
        &self,
        name: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<&Value>> {
        self.attrs.get(name, heap, interns)
    }

    /// Sets an attribute value.
    ///
    /// The caller transfers ownership of both `name` and `value`. Returns the
    /// old value if the attribute existed (caller must drop it), or None if this
    /// is a new attribute.
    ///
    /// Returns `FrozenInstanceError` if the dataclass is frozen.
    pub fn set_attr(
        &mut self,
        name: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        if self.frozen {
            // Get attribute name for error message
            let attr_name = match &name {
                Value::InternString(id) => interns.get_str(*id).to_string(),
                _ => "<unknown>".to_string(),
            };
            // Drop the values we were given ownership of
            name.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(ExcType::frozen_instance_error(&attr_name));
        }
        self.attrs.set(name, value, heap, interns)
    }

    /// Computes the hash for this dataclass if it's frozen.
    ///
    /// Returns Some(hash) for frozen (immutable) dataclasses, None for mutable ones.
    /// The hash is computed from the class name and declared field values only.
    pub fn compute_hash(&self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Option<u64> {
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };

        // Only frozen (immutable) dataclasses are hashable
        if !self.frozen {
            return None;
        }

        let mut hasher = DefaultHasher::new();
        // Hash the class name
        self.name.hash(&mut hasher);
        // Hash each declared field (name, value) pair in order
        for field_name in &self.field_names {
            field_name.hash(&mut hasher);
            if let Some(value) = self.attrs.get_by_str(field_name, heap, interns) {
                let value_hash = value.py_hash(heap, interns)?;
                value_hash.hash(&mut hasher);
            }
        }
        Some(hasher.finish())
    }
}

impl PyTrait for Dataclass {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Dataclass
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.len()
            + self.field_names.iter().map(String::len).sum::<usize>()
            + self.attrs.py_estimate_size()
            + self.methods.len() * std::mem::size_of::<String>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Dataclasses don't have a length
        None
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        // Dataclasses are equal if they have the same name and equal attrs
        self.name == other.name && self.attrs.py_eq(&other.attrs, heap, interns)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Delegate to the attrs Dict which handles all nested heap references
        self.attrs.py_dec_ref_ids(stack);
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Dataclass instances are always truthy (like Python objects)
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        heap: &Heap<impl ResourceTracker>,
        heap_ids: &mut AHashSet<HeapId>,
        interns: &Interns,
    ) -> std::fmt::Result {
        // Format: ClassName(field1=value1, field2=value2, ...)
        // Only declared fields are shown, not dynamically added attributes
        f.write_str(&self.name)?;
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
            if let Some(value) = self.attrs.get_by_str(field_name, heap, interns) {
                value.py_repr_fmt(f, heap, heap_ids, interns)?;
            } else {
                // Field not found - shouldn't happen for well-formed dataclasses
                f.write_str("<?>")?;
            }
        }

        f.write_char(')')
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        // Get method name from the attribute
        let method_name = attr.as_str(interns);

        if self.methods.contains(method_name) {
            // TODO: Integrate with external call system
            // For now, drop args and return an error indicating this needs implementation
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error_method_not_implemented(&self.name, method_name))
        } else {
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error(Type::Dataclass, method_name))
        }
    }
}

// Custom serde implementation for Dataclass.
// Serializes all six fields; methods set is serialized as a Vec for determinism.
impl serde::Serialize for Dataclass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Dataclass", 6)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("type_id", &self.type_id)?;
        state.serialize_field("field_names", &self.field_names)?;
        state.serialize_field("attrs", &self.attrs)?;
        // Serialize methods as sorted Vec for deterministic output
        let mut methods_vec: Vec<&String> = self.methods.iter().collect();
        methods_vec.sort();
        state.serialize_field("methods", &methods_vec)?;
        state.serialize_field("frozen", &self.frozen)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Dataclass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct DataclassData {
            name: String,
            type_id: u64,
            field_names: Vec<String>,
            attrs: Dict,
            methods: Vec<String>,
            frozen: bool,
        }
        let dc = DataclassData::deserialize(deserializer)?;
        Ok(Self {
            name: dc.name,
            type_id: dc.type_id,
            field_names: dc.field_names,
            attrs: dc.attrs,
            methods: dc.methods.into_iter().collect(),
            frozen: dc.frozen,
        })
    }
}
