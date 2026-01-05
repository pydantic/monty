use std::fmt::Write;

use ahash::AHashSet;

use crate::args::ArgValues;
use crate::exception_private::ExcType;
use crate::types::Type;

use super::{Dict, PyTrait};
use crate::heap::{Heap, HeapData, HeapId};
use crate::intern::Interns;
use crate::resource::ResourceTracker;
use crate::run_frame::RunResult;
use crate::value::{Attr, Value};

/// Python dataclass instance type.
///
/// Represents an instance of a dataclass with a class name, field values, and
/// a set of method names that trigger external function calls when invoked.
///
/// # Fields
/// - `name`: The class name (e.g., "Point", "User")
/// - `fields`: A Dict mapping field names (strings) to their values
/// - `methods`: Set of method names that should trigger external calls
/// - `mutable`: Whether the dataclass instance can be modified
///
/// # Hashability
/// When `mutable` is false, the dataclass is immutable and hashable. The hash
/// is computed from the class name and all field values. When `mutable` is true,
/// the dataclass behaves like a regular Python object and is unhashable.
///
/// # Reference Counting
/// The `fields` Dict contains Values that may be heap-allocated. The
/// `py_dec_ref_ids` method properly handles decrementing refcounts for
/// all field values when the dataclass instance is freed.
///
/// # Attribute Access
/// - Getting: Looks up the field name in the fields Dict
/// - Setting: Updates or adds the field in the fields Dict (only if mutable)
/// - Method calls: If the attribute name is in `methods`, triggers external call
#[derive(Debug)]
pub struct Dataclass {
    /// The class name (e.g., "Point", "User")
    name: String,
    /// Field name -> value mapping, preserves insertion order
    fields: Dict,
    /// Method names that trigger external function calls
    methods: AHashSet<String>,
    /// Whether this dataclass instance is mutable (affects hashability)
    mutable: bool,
}

impl Dataclass {
    /// Creates a new dataclass instance.
    ///
    /// # Arguments
    /// * `name` - The class name
    /// * `fields` - Dict of field name -> value pairs (ownership transferred)
    /// * `methods` - Set of method names that trigger external calls
    /// * `mutable` - Whether this instance is mutable (affects hashability)
    #[must_use]
    pub fn new(name: String, fields: Dict, methods: AHashSet<String>, mutable: bool) -> Self {
        Self {
            name,
            fields,
            methods,
            mutable,
        }
    }

    /// Returns the class name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a reference to the methods set.
    #[must_use]
    pub fn methods(&self) -> &AHashSet<String> {
        &self.methods
    }

    /// Returns a reference to the fields Dict.
    #[must_use]
    pub fn fields(&self) -> &Dict {
        &self.fields
    }

    /// Returns whether this dataclass instance is mutable.
    #[must_use]
    pub fn is_mutable(&self) -> bool {
        self.mutable
    }

    /// Gets a field value by name.
    ///
    /// Returns Ok(Some(&Value)) if the field exists, Ok(None) if it doesn't.
    /// Returns Err if the key is unhashable (should not happen with string keys).
    pub fn get_field(
        &self,
        name: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<&Value>> {
        self.fields.get(name, heap, interns)
    }

    /// Sets a field value.
    ///
    /// The caller transfers ownership of both `name` and `value`. Returns the
    /// old value if the field existed (caller must drop it), or None if this
    /// is a new field.
    pub fn set_field(
        &mut self,
        name: Value,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<Value>> {
        self.fields.set(name, value, heap, interns)
    }

    /// Checks if a method name is in the methods set.
    #[must_use]
    pub fn has_method(&self, name: &str) -> bool {
        self.methods.contains(name)
    }

    /// Creates a deep clone of this dataclass with proper reference counting.
    ///
    /// The fields Dict is cloned with proper refcount handling for all values.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<impl ResourceTracker>) -> Self {
        Self {
            name: self.name.clone(),
            fields: self.fields.clone_with_heap(heap),
            methods: self.methods.clone(),
            mutable: self.mutable,
        }
    }

    /// Computes the hash for this dataclass if it's immutable.
    ///
    /// Returns Some(hash) for immutable dataclasses, None for mutable ones.
    /// The hash is computed from the class name and all field values.
    pub fn compute_hash(&self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Option<u64> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        if self.mutable {
            return None;
        }

        let mut hasher = DefaultHasher::new();
        // Hash the class name
        self.name.hash(&mut hasher);
        // Hash each field (name, value) pair
        for (key, value) in &self.fields {
            // Hash the key
            let key_hash = key.py_hash(heap, interns)?;
            key_hash.hash(&mut hasher);
            // Hash the value
            let value_hash = value.py_hash(heap, interns)?;
            value_hash.hash(&mut hasher);
        }
        Some(hasher.finish())
    }
}

impl PyTrait for Dataclass {
    fn py_type(&self, _heap: Option<&Heap<impl ResourceTracker>>) -> Type {
        Type::Dataclass
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.len()
            + self.fields.py_estimate_size()
            + self.methods.len() * std::mem::size_of::<String>()
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Dataclasses don't have a length
        None
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> bool {
        // Dataclasses are equal if they have the same name and equal fields
        self.name == other.name && self.fields.py_eq(&other.fields, heap, interns)
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        // Delegate to the fields Dict which handles all nested heap references
        self.fields.py_dec_ref_ids(stack);
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
        f.write_str(&self.name)?;
        f.write_char('(')?;

        let mut first = true;
        for (key, value) in &self.fields {
            if !first {
                f.write_str(", ")?;
            }
            first = false;

            // Write field name (should be a string, write without quotes)
            match key {
                Value::InternString(id) => f.write_str(interns.get_str(*id))?,
                Value::Ref(id) => {
                    if let HeapData::Str(s) = heap.get(*id) {
                        f.write_str(s.as_str())?;
                    } else {
                        key.py_repr_fmt(f, heap, heap_ids, interns)?;
                    }
                }
                _ => key.py_repr_fmt(f, heap, heap_ids, interns)?,
            }

            f.write_char('=')?;
            value.py_repr_fmt(f, heap, heap_ids, interns)?;
        }

        f.write_char(')')
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        _interns: &Interns,
    ) -> RunResult<Value> {
        // Check if this is a method that should trigger an external call
        let method_name = match attr {
            Attr::Other(name) => name.as_str(),
            _ => return Err(ExcType::attribute_error(Type::Dataclass, attr)),
        };

        if self.methods.contains(method_name) {
            // TODO: Integrate with external call system
            // For now, drop args and return an error indicating this needs implementation
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error_method_not_implemented(&self.name, method_name))
        } else {
            args.drop_with_heap(heap);
            Err(ExcType::attribute_error(Type::Dataclass, attr))
        }
    }
}

// Custom serde implementation for Dataclass.
// Serializes all four fields; methods set is serialized as a Vec for determinism.
impl serde::Serialize for Dataclass {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Dataclass", 4)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("fields", &self.fields)?;
        // Serialize methods as sorted Vec for deterministic output
        let mut methods_vec: Vec<&String> = self.methods.iter().collect();
        methods_vec.sort();
        state.serialize_field("methods", &methods_vec)?;
        state.serialize_field("mutable", &self.mutable)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Dataclass {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct DataclassFields {
            name: String,
            fields: Dict,
            methods: Vec<String>,
            mutable: bool,
        }
        let dc = DataclassFields::deserialize(deserializer)?;
        Ok(Self {
            name: dc.name,
            fields: dc.fields,
            methods: dc.methods.into_iter().collect(),
            mutable: dc.mutable,
        })
    }
}
