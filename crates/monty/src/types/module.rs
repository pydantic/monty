//! Python module type for representing imported modules.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{HeapGuard, HeapId, HeapRead},
    intern::StringId,
    resource::ResourceTracker,
    types::{Dict, PyTrait},
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
    /// Dispatches a method call on a heap-allocated module via the `HeapRead` pattern.
    ///
    /// Uses `get_by_str` (which only needs `&Heap`) for the attribute lookup, then
    /// clones the found value via a short-lived borrow before calling the function.
    pub(crate) fn call_attr(
        self,
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
