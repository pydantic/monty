//! Python module type for representing imported modules.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{HeapGuard, HeapId, HeapItem, HeapRead},
    intern::StringId,
    resource::ResourceTracker,
    types::Dict,
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
    pub fn set_attr(&mut self, name: impl Into<StringId>, value: Value, vm: &mut VM<'_, '_, impl ResourceTracker>) {
        let key = Value::InternString(name.into());
        // Unwrap is safe because InternString keys are always hashable
        self.attrs.set(key, value, vm).unwrap();
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

impl<'h> HeapRead<'h, Module> {
    /// Gets an attribute by string ID for the `py_getattr` trait method.
    ///
    /// Returns the attribute value if found, or `None` if the attribute doesn't exist.
    /// For `Property` values, invokes the property getter rather than returning
    /// the Property itself - this implements Python's descriptor protocol.
    pub fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h, '_, impl ResourceTracker>) -> Option<CallResult> {
        let value = self
            .get(vm.heap)
            .attrs
            .get_by_str(attr.as_str(vm.interns), vm.heap, vm.interns)?;

        // If the value is a Property, invoke its getter to compute the actual value
        if let Value::Property(prop) = *value {
            Some(prop.get())
        } else {
            Some(CallResult::Value(value.clone_with_heap(vm)))
        }
    }

    /// Dispatches a method call on a heap-allocated module via the `HeapRead` pattern.
    ///
    /// Uses `get_by_str` (which only needs `&Heap`) for the attribute lookup, then
    /// clones the found value via a short-lived borrow before calling the function.
    /// Alias for `call_attr` to match the `PyTrait` naming convention used by
    /// `heap_read_call_attr` in `heap_data.rs`.
    pub(crate) fn py_call_attr(
        &mut self,
        self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        self.call_attr(self_id, vm, attr, args)
    }

    /// Dispatches a method call on a heap-allocated module via the `HeapRead` pattern.
    ///
    /// Uses `get_by_str` (which only needs `&Heap`) for the attribute lookup, then
    /// clones the found value via a short-lived borrow before calling the function.
    pub(crate) fn call_attr(
        &self,
        _self_id: HeapId,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
    ) -> RunResult<CallResult> {
        let mut args_guard = HeapGuard::new(args, vm);
        let vm = args_guard.heap();

        // Module name is Copy (just a StringId)
        let module_name = self.get(vm.heap).name();

        let attr_str = match attr {
            EitherStr::Interned(id) => vm.interns.get_str(*id),
            EitherStr::Heap(s) => {
                return Err(ExcType::attribute_error_module(vm.interns.get_str(module_name), s));
            }
        };

        // Look up via get_by_str — only needs &Heap (immutable), so compatible with
        // the HeapRead borrow. Clone the value using short-lived borrow pattern.
        let ref_id = {
            match self.get(vm.heap).attrs().get_by_str(attr_str, vm.heap, vm.interns) {
                Some(Value::Ref(id)) => Some(Some(*id)),
                Some(_) => Some(None),
                None => None,
            }
        };

        match ref_id {
            Some(Some(id)) => {
                // Ref value — inc_ref outside the borrow, then call
                vm.heap.inc_ref(id);
                let value = Value::Ref(id);
                let (args, vm) = args_guard.into_parts();
                defer_drop!(value, vm);
                vm.call_function(value, args)
            }
            Some(None) => {
                // Non-ref value (e.g., Int, Bool) — clone without heap access
                let value = self
                    .get(vm.heap)
                    .attrs()
                    .get_by_str(attr_str, vm.heap, vm.interns)
                    .unwrap()
                    .clone_immediate();
                let (args, vm) = args_guard.into_parts();
                defer_drop!(value, vm);
                vm.call_function(value, args)
            }
            None => Err(ExcType::attribute_error_module(
                vm.interns.get_str(module_name),
                attr.as_str(vm.interns),
            )),
        }
    }
}

impl HeapItem for Module {
    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.attrs.py_estimate_size()
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.attrs.py_dec_ref_ids(stack);
    }
}
