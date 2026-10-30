//! Attribute access helpers for the VM.

use super::VM;
use crate::{
    bytecode::{Opcode, vm::CallResult},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError},
    heap::{ContainsHeap, DropWithContext, Heap, HeapData},
    intern::{Interns, StringId},
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
    /// A class-pattern attribute read (`MatchAttr`): pushes the served value
    /// (or `None` for `Undefined`) itself and yields the match flag, so the
    /// resumed instruction lands the same `[value, bool]` pair it pushes
    /// when the attribute is answered synchronously.
    MatchAttr,
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
            (Self::MatchAttr, Some(value)) => {
                vm.push(value);
                Value::Bool(true)
            }
            (Self::MatchAttr, None) => {
                vm.push(Value::None);
                Value::Bool(false)
            }
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

    /// Loads a `from ... import` attribute, raising `ImportError` if missing.
    /// A file package's missing attribute may be a submodule, imported here.
    /// `module_id` supplies the error's module name because host values need not be `Module`s.
    pub(super) fn load_attr_import(&mut self, name_id: StringId, module_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        match obj.py_getattr(&attr, this) {
            Ok(result) => Ok(result),
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                this.import_file_attr(obj, name_id).unwrap_or_else(|| {
                    let name_str = this.interns.get_str(name_id);
                    Err(ExcType::cannot_import_name(
                        name_str,
                        this.interns.get_str(module_id),
                        None,
                    ))
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Replaces the stack top with the next submodule of a dotted import.
    /// Missing attributes raise `ModuleNotFoundError` naming `path_id`. Modules also
    /// reject non-module attributes (`math.pi`, the host-served `os.environ`); bundled
    /// ones add "is not a package" to errors, host modules being packages. A file
    /// package imports the submodule's own file, whatever attribute it has.
    pub(super) fn load_submodule(&mut self, name_id: StringId, path_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        if let Some(result) = this.import_file_submodule(obj, path_id) {
            return result;
        }
        let package = module_package(obj, this.heap);
        let is_builtin = package == Some(false);
        let attr = EitherStr::Interned(name_id);
        match obj.py_getattr(&attr, this) {
            Ok(CallResult::Value(value)) if package.is_some() && module_package(&value, this.heap).is_some() => {
                Ok(CallResult::Value(value))
            }
            // a property's OS call (`os.environ`) is no more a submodule than a value
            Ok(result) if package.is_some() => {
                result.drop_with(this);
                Err(not_a_submodule(this.interns, path_id, is_builtin))
            }
            Ok(result) => Ok(result),
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                Err(not_a_submodule(this.interns, path_id, is_builtin))
            }
            Err(e) => Err(e),
        }
    }

    /// Returns the suspended `LoadSubmodule` path for errors from lazy attribute lookups.
    pub(crate) fn suspended_submodule(&self) -> Option<StringId> {
        let ip = self.instruction_ip;
        let bytecode = self.current_frame.bytecode;
        (bytecode.get(ip) == Some(&(Opcode::LoadSubmodule as u8))).then(|| {
            // operands: u16 attribute name_id, then u16 path name_id, little-endian
            StringId::from_index(u16::from_le_bytes([bytecode[ip + 3], bytecode[ip + 4]]))
        })
    }

    /// Returns the suspended `LoadAttrImport` module for errors from lazy attribute lookups.
    pub(crate) fn suspended_import_from(&self) -> Option<StringId> {
        let frame = &self.current_frame;
        (frame.bytecode.get(self.instruction_ip) == Some(&(Opcode::LoadAttrImport as u8)))
            .then_some(frame.import_from_module)
            .flatten()
    }

    /// Returns the suspended `LoadModule` name, distinguishing imports from calls to `__import__`.
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

/// `Some(is_package)` when `value` is a module: a host module is a package
/// (its submodules are module attributes), a bundled one is not.
fn module_package(value: &Value, heap: &Heap) -> Option<bool> {
    match value {
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Module(module) => Some(module.id().is_some()),
            _ => None,
        },
        _ => None,
    }
}

/// Builds a missing-submodule error using the import's spelling of a built-in parent (`os.path`).
fn not_a_submodule(interns: &Interns, path_id: StringId, is_builtin: bool) -> RunError {
    let path = interns.get_str(path_id);
    let parent = is_builtin.then(|| path.rsplit_once('.').map_or(path, |(parent, _)| parent));
    ExcType::module_not_found_error(path, parent)
}
