//! Attribute access helpers for the VM.

use super::VM;
use crate::{
    bytecode::{Opcode, vm::CallResult},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError},
    heap::{ContainsHeap, DropWithContext, HeapData},
    intern::StringId,
    value::{EitherStr, Value},
};

/// What a suspended lazy attribute lookup does with the host's answer when it
/// resumes, instead of pushing the value (or raising `AttributeError` on
/// `Undefined`) the way `obj.attr` does.
///
/// Produced by the `getattr()` / `hasattr()` builtins. It rides on
/// [`CallResult::AttrLookup`] and `FrameExit::AttrLookup`, and is armed on
/// [`VM::pending_lookup_effect`] once the lookup reaches the host, so it
/// survives a dump/restore of the suspended session. A lookup that never
/// reaches a host (no host, or a synchronous nested call) is `Undefined`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum PendingLookupEffect {
    /// `hasattr()`: push `True` for a served value (which is dropped),
    /// `False` for `Undefined`.
    HasAttr,
    /// `getattr(obj, name, default)`: push `default` for `Undefined`. Owns
    /// the heap reference until the resume consumes it.
    Default(Value),
}

impl PendingLookupEffect {
    /// The value to push for the host's answer: `Some(value)` for a served
    /// attribute, `None` for `Undefined`.
    pub(crate) fn apply(self, answer: Option<Value>, vm: &mut VM<'_>) -> Value {
        match (self, answer) {
            (Self::HasAttr, Some(value)) => {
                value.drop_with(vm);
                Value::Bool(true)
            }
            (Self::HasAttr, None) => Value::Bool(false),
            (Self::Default(default), Some(value)) => {
                default.drop_with(vm);
                value
            }
            (Self::Default(default), None) => default,
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for PendingLookupEffect {
    fn drop_with(self, heap: &mut C) {
        if let Self::Default(value) = self {
            value.drop_with(heap);
        }
    }
}

impl VM<'_> {
    /// Loads an attribute from an object and pushes it onto the stack.
    ///
    /// Returns an AttributeError if the attribute doesn't exist.
    pub(super) fn load_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        obj.py_getattr(&attr, this)
    }

    /// Loads an attribute from a module for `from ... import` and pushes it onto the stack.
    ///
    /// Returns an ImportError (not AttributeError) if the attribute doesn't exist,
    /// matching CPython's behavior for `from module import name`. `module_id` (the
    /// frame's `import_from_module`) names the module for that message, since a
    /// host-provided module need not be a `Module`.
    pub(super) fn load_attr_import(&mut self, name_id: StringId, module_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        match obj.py_getattr(&attr, this) {
            Ok(result) => Ok(result),
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                let name_str = this.interns.get_str(name_id);
                Err(ExcType::cannot_import_name(name_str, this.interns.get_str(module_id)))
            }
            Err(e) => Err(e),
        }
    }

    /// Loads the next component of a dotted import: pops the module so far and
    /// pushes its attribute `name_id`. A missing attribute is
    /// `ModuleNotFoundError` naming `path_id` (`a.b`), with `; 'a' is not a
    /// package` when the module is a built-in one, since a built-in module has
    /// no submodules a host could supply.
    pub(super) fn load_submodule(&mut self, name_id: StringId, path_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        match obj.py_getattr(&attr, this) {
            Ok(result) => Ok(result),
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                let path = this.interns.get_str(path_id);
                // the parent as spelled in the import (`os.path`), not the module's own name
                let is_builtin = matches!(obj, Value::Ref(id) if matches!(this.heap.get(*id), HeapData::Module(_)));
                let parent = is_builtin.then(|| path.rsplit_once('.').map_or(path, |(parent, _)| parent));
                Err(ExcType::module_not_found_error(path, parent))
            }
            Err(e) => Err(e),
        }
    }

    /// The dotted path a suspended `LoadSubmodule` is loading (`a.b`): `Some`
    /// while the suspended instruction is one, so a host's answer to the lazy
    /// attribute lookup can raise the same `ModuleNotFoundError` the synchronous
    /// load does.
    pub(crate) fn suspended_submodule(&self) -> Option<StringId> {
        let ip = self.instruction_ip;
        let bytecode = self.current_frame.bytecode;
        (bytecode.get(ip) == Some(&(Opcode::LoadSubmodule as u8))).then(|| {
            // operands: u16 attribute name_id, then u16 path name_id, little-endian
            StringId::from_index(u16::from_le_bytes([bytecode[ip + 3], bytecode[ip + 4]]))
        })
    }

    /// The module a suspended `from <module> import <name>` is loading from: `Some`
    /// while the suspended instruction is its attribute load, so a host's answer to
    /// that lookup can raise the same `ImportError` the synchronous load does.
    pub(crate) fn suspended_import_from(&self) -> Option<StringId> {
        let frame = &self.current_frame;
        (frame.bytecode.get(self.instruction_ip) == Some(&(Opcode::LoadAttrImport as u8)))
            .then_some(frame.import_from_module)
            .flatten()
    }

    /// The module a suspended `import <module>` is loading: `Some` while the suspended
    /// instruction is its `LoadModule`. This is what distinguishes an import's host call
    /// from a direct call of an undefined `__import__`.
    pub(crate) fn suspended_import(&self) -> Option<StringId> {
        let ip = self.instruction_ip;
        let bytecode = self.current_frame.bytecode;
        (bytecode.get(ip) == Some(&(Opcode::LoadModule as u8))).then(|| {
            // operand: u16 module name_id, little-endian
            StringId::from_index(u16::from_le_bytes([bytecode[ip + 1], bytecode[ip + 2]]))
        })
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
        obj.py_set_attr(&EitherStr::Interned(name_id), value, this)
    }
}
