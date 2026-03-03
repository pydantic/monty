//! Attribute access helpers for the VM.

use super::VM;
use crate::{
    bytecode::vm::CallResult,
    defer_drop,
    exception_private::{ExcType, RunError},
    intern::StringId,
    resource::ResourceTracker,
    value::EitherStr,
};

impl<T: ResourceTracker> VM<'_, '_, T> {
    /// Loads an attribute from an object and pushes it onto the stack.
    ///
    /// Returns an AttributeError if the attribute doesn't exist.
    pub(super) fn load_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        let result = obj.py_getattr(&attr, this.heap, this.interns)?;
        Ok(result.into())
    }

    /// Loads an attribute from a module for `from ... import` and pushes it onto the stack.
    ///
    /// Returns an ImportError (not AttributeError) if the attribute doesn't exist,
    /// matching CPython's behavior for `from module import name`.
    pub(super) fn load_attr_import(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        match obj.py_getattr(&attr, this.heap, this.interns) {
            Ok(result) => Ok(result.into()),
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                // Only compute module_name when we need it for the error message
                let module_name = obj.module_name(this.heap, this.interns);
                let name_str = this.interns.get_str(name_id);
                Err(ExcType::cannot_import_name(name_str, &module_name))
            }
            Err(e) => Err(e),
        }
    }

    /// Stores a value as an attribute on an object.
    ///
    /// Returns an AttributeError if the attribute cannot be set.
    pub(super) fn store_attr(&mut self, name_id: StringId) -> Result<(), RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let value = this.pop();
        // py_set_attr takes ownership of value and drops it on error
        obj.py_set_attr(name_id, value, this.heap, this.interns)
    }
}
