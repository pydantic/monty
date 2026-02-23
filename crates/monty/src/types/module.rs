//! Python module type for representing imported modules.

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapGuard, HeapId},
    intern::{Interns, StringId},
    resource::ResourceTracker,
    types::{AttrCallResult, Dict, PyTrait},
    value::{EitherStr, Value},
};

/// A Python module with a name and attribute dictionary.
///
/// Modules in Monty are simplified compared to CPython - they just have a name
/// and a dictionary of attributes. This is sufficient for built-in modules like
/// `sys` and `typing` where we control the available attributes.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Module {
    /// The module name (e.g., "sys", "typing").
    name: StringId,
    /// The module's attributes (e.g., `version`, `platform` for `sys`).
    attrs: Dict,
}

impl Module {
    /// Creates a new module with an empty attributes dictionary.
    ///
    /// The module name must be pre-interned during the prepare phase.
    ///
    /// # Panics
    ///
    /// Panics if the module name string has not been pre-interned.
    pub fn new(name: impl Into<StringId>) -> Self {
        Self {
            name: name.into(),
            attrs: Dict::new(),
        }
    }

    /// Returns the module's name StringId.
    pub fn name(&self) -> StringId {
        self.name
    }

    /// Returns a reference to the module's attribute dictionary.
    pub fn attrs(&self) -> &Dict {
        &self.attrs
    }

    /// Sets an attribute in the module's dictionary.
    ///
    /// The attribute name must be pre-interned during the prepare phase.
    ///
    /// # Panics
    ///
    /// Panics if the attribute name string has not been pre-interned.
    pub fn set_attr(
        &mut self,
        name: impl Into<StringId>,
        value: Value,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) {
        let key = Value::InternString(name.into());
        // Unwrap is safe because InternString keys are always hashable
        self.attrs.set(key, value, heap, interns).unwrap();
    }

    /// Looks up an attribute by name in the module's attribute dictionary.
    ///
    /// Returns `Some(value)` if the attribute exists, `None` otherwise.
    /// The returned value is cloned with proper refcount handling.
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
            .map(|v| v.clone_with_heap(heap))
    }

    /// Returns whether this module has any heap references in its attributes.
    pub fn has_refs(&self) -> bool {
        self.attrs.has_refs()
    }

    /// Collects child HeapIds for reference counting.
    pub fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.attrs.py_dec_ref_ids(stack);
    }

    /// Gets an attribute by string ID for the `py_getattr` trait method.
    ///
    /// Returns the attribute value if found, or `None` if the attribute doesn't exist.
    /// For `Property` values, invokes the property getter rather than returning
    /// the Property itself - this implements Python's descriptor protocol.
    pub fn py_getattr(
        &self,
        attr_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> Option<AttrCallResult> {
        let value = self.attrs.get_by_str(interns.get_str(attr_id), heap, interns)?;

        // If the value is a Property, invoke its getter to compute the actual value
        if let Value::Property(prop) = *value {
            Some(prop.get())
        } else {
            Some(AttrCallResult::Value(value.clone_with_heap(heap)))
        }
    }

    /// Calls an attribute as a function on this module.
    ///
    /// Modules don't have methods - they have callable attributes. This looks up
    /// the attribute and calls it if it's a `ModuleFunction`.
    ///
    /// Returns `AttrCallResult` because module functions may need OS operations
    /// (e.g., `os.getenv()`) that require host involvement.
    pub fn py_call_attr_raw(
        &self,
        _self_id: HeapId,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<AttrCallResult> {
        let heap = &mut *vm.heap;
        let interns = vm.interns;
        let mut args_guard = HeapGuard::new(args, heap);

        let attr_key = match attr {
            EitherStr::Interned(id) => Value::InternString(*id),
            EitherStr::Heap(s) => {
                // Module attributes are always interned, so owned strings won't match
                return Err(ExcType::attribute_error_module(interns.get_str(self.name), s));
            }
        };

        match self.get_attr(&attr_key, args_guard.heap(), interns) {
            Some(Value::ModuleFunction(mf)) => {
                let (args, heap) = args_guard.into_parts();
                mf.call(heap, args)
            }
            Some(func) => {
                // Found attribute but it's not callable
                func.drop_with_heap(args_guard.heap());
                Err(ExcType::type_error("module attribute is not callable"))
            }
            None => Err(ExcType::attribute_error_module(
                interns.get_str(self.name),
                attr.as_str(vm.interns),
            )),
        }
    }
}
