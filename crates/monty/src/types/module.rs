//! Python module type for representing imported modules.

use crate::{
    heap::{Heap, HeapId},
    intern::{Interns, StringId},
    resource::ResourceTracker,
    types::{Dict, PyTrait},
    value::Value,
};

/// A Python module with a name and attribute dictionary.
///
/// Modules in Monty are simplified compared to CPython - they just have a name
/// and a dictionary of attributes. This is sufficient for built-in modules like
/// `sys` and `typing` where we control the available attributes.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Module {
    /// The module name (e.g., "sys", "typing").
    name: StringId,
    /// The module's attributes (e.g., `sys.version`, `sys.platform`).
    attrs: Dict,
}

impl Module {
    /// Creates a new module with the given name and attributes.
    pub fn new(name: StringId, attrs: Dict) -> Self {
        Self { name, attrs }
    }

    /// Returns the module's name StringId.
    pub fn name(&self) -> StringId {
        self.name
    }

    /// Returns a reference to the module's attribute dictionary.
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }

    /// Looks up an attribute by name in the module's attribute dictionary.
    ///
    /// Returns `Some(value)` if the attribute exists, `None` otherwise.
    /// The returned value is copied without incrementing refcount - caller must
    /// call `heap.inc_ref()` if the value is a `Value::Ref`.
    pub fn get_attr(
        &self,
        attr_value: &Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Option<Value> {
        // Dict::get returns Result because of hash computation, but InternString keys
        // are always hashable, so unwrap is safe here.
        self.attrs
            .get(attr_value, heap, interns)
            .ok()
            .flatten()
            .map(Value::copy_for_extend)
    }

    /// Returns whether this module has any heap references in its attributes.
    pub fn has_refs(&self) -> bool {
        self.attrs.has_refs()
    }

    /// Collects child HeapIds for reference counting.
    pub fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.attrs.py_dec_ref_ids(stack);
    }
}
