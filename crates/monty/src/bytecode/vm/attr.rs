//! Attribute access helpers for the VM.

use super::VM;
use crate::{
    bytecode::vm::CallResult,
    exception_private::{ExcType, RunError},
    intern::StringId,
    io::PrintWriter,
    resource::ResourceTracker,
};

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Loads an attribute from an object and pushes it onto the stack.
    ///
    /// Returns an AttributeError if the attribute doesn't exist.
    pub(super) fn load_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let obj = self.pop();
        let result = obj.py_getattr(name_id, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        // Convert AttrCallResult to CallResult
        result.map(Into::into)
    }

    /// Loads an attribute from a module for `from ... import` and pushes it onto the stack.
    ///
    /// Returns an ImportError (not AttributeError) if the attribute doesn't exist,
    /// matching CPython's behavior for `from module import name`.
    pub(super) fn load_attr_import(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let obj = self.pop();
        let result = obj.py_getattr(name_id, self.heap, self.interns);
        match result {
            Ok(result) => {
                obj.drop_with_heap(self.heap);
                Ok(result.into())
            }
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                // Only compute module_name when we need it for the error message
                let module_name = obj.module_name(self.heap, self.interns);
                obj.drop_with_heap(self.heap);
                let name_str = self.interns.get_str(name_id);
                Err(ExcType::cannot_import_name(name_str, &module_name))
            }
            Err(e) => {
                obj.drop_with_heap(self.heap);
                Err(e)
            }
        }
    }

    /// Stores a value as an attribute on an object.
    ///
    /// Returns an AttributeError if the attribute cannot be set.
    pub(super) fn store_attr(&mut self, name_id: StringId) -> Result<(), RunError> {
        let obj = self.pop();
        let value = self.pop();
        // py_set_attr takes ownership of value and drops it on error
        let result = obj.py_set_attr(name_id, value, self.heap, self.interns);
        obj.drop_with_heap(self.heap);
        result
    }
}
