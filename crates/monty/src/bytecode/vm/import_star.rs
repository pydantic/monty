//! `from m import *`: listing the names a module exports and binding each by name.
//!
//! The names are only known at runtime, so the compiler emits a loop over them
//! (see `Compiler::compile_import_star`) whose three steps are the modes of
//! `Opcode::ImportStar`. `NAMES` reads `__all__` up front, stopping at its first
//! non-`str` entry, which `LOAD` rejects once the names before it are bound.

use super::VM;
use crate::{
    bytecode::{CallResult, IMPORT_STAR_LOAD, IMPORT_STAR_NAMES, vm::FrameGlobals},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    name_map::namespace_overflow_message,
    namespace::NamespaceId,
    parse::CodeRange,
    types::{Dict, List, Module, PyTrait},
    value::Value,
};

impl VM<'_> {
    /// `ImportStar` in the `IMPORT_STAR_NAMES` or `IMPORT_STAR_LOAD` mode,
    /// whose result the dispatch loop pushes (or suspends for).
    pub(super) fn import_star_value(&mut self, mode: u8) -> RunResult<CallResult> {
        match mode {
            IMPORT_STAR_NAMES => self.import_star_names().map(CallResult::Value),
            IMPORT_STAR_LOAD => self.import_star_load(),
            _ => Err(RunError::internal("invalid ImportStar mode")),
        }
    }

    /// `[module] -> [module, names]`: a list of the entries of `module.__all__`,
    /// or of every key of its namespace that does not start with `_`.
    ///
    /// Entries are not checked here: `import_star_load` raises CPython's
    /// `TypeError` for a non-`str` one when the loop reaches it.
    fn import_star_names(&mut self) -> RunResult<Value> {
        let module = self.import_star_module(0)?;
        let attrs = module_attrs(module, self)?;
        let names = match attrs.get_by_str("__all__", self.heap, self.interns) {
            Some(all) => {
                let all = all.clone_with_heap(self.heap);
                let this = self;
                defer_drop!(all, this);
                return this.import_star_all_names(all);
            }
            None => attrs
                .into_iter()
                .filter(|(key, _)| !key.as_str(self).is_some_and(|name| name.starts_with('_')))
                .map(|(key, _)| key.clone_with_heap(self.heap))
                .collect(),
        };
        Ok(Value::Ref(self.heap.allocate(HeapData::List(List::new(names)))))
    }

    /// The entries of a module's `__all__`, read as CPython reads them: by
    /// index from 0 until `IndexError`, so a class with `__getitem__` works.
    ///
    /// Reading stops after the first non-`str` entry, which `import_star_load`
    /// rejects once the names before it are bound: indexing on could never end,
    /// as a `Counter` answers every index.
    fn import_star_all_names(&mut self, all: &Value) -> RunResult<Value> {
        if !self.has_getitem_method(all)
            && let Some(error) = ExcType::sequence_index_error(all, self)
        {
            return Err(ExcType::import_star_all_not_sequence(error));
        }
        let mut names_guard = DropGuard::new(Vec::new(), self);
        let (names, this) = names_guard.as_parts_mut();
        for index in 0_i64.. {
            let item = match all.py_getitem(&Value::Int(index), this) {
                Ok(item) => item,
                Err(error) if error.is_catchable(ExcType::IndexError) => break,
                Err(error) => return Err(error),
            };
            let is_str = item.as_str(this).is_some();
            this.heap
                .tracker
                .check_growth(names.len(), names.capacity(), size_of::<Value>())?;
            names.push(item);
            if !is_str {
                break;
            }
        }
        let (names, this) = names_guard.into_parts();
        Ok(Value::Ref(this.heap.allocate(HeapData::List(List::new(names)))))
    }

    /// Whether `value` is an instance whose class defines `__getitem__`, which
    /// gives it CPython's `sq_item`.
    fn has_getitem_method(&self, value: &Value) -> bool {
        let Value::Ref(id) = value else { return false };
        let HeapData::Instance(instance) = self.heap.get(*id) else {
            return false;
        };
        matches!(
            self.heap.get(instance.class()),
            HeapData::Class(class) if class.namespace().get_by_str("__getitem__", self.heap, self.interns).is_some()
        )
    }

    /// `[module, iter, name] -> [module, iter, name, value]`: reads the
    /// attribute as `getattr()` would, so a property can suspend for the host.
    fn import_star_load(&mut self) -> RunResult<CallResult> {
        let len = self.stack.len();
        let Some(attr) = self.stack[len - 1].as_either_str(self.heap) else {
            let module = self.import_star_module(2)?;
            let attrs = module_attrs(module, self)?;
            let from_all = attrs.get_by_str("__all__", self.heap, self.interns).is_some();
            let module_name = module
                .error_name(self)
                .map_err(|name| ExcType::import_star_module_name_type(&name.py_type_name(self)))?;
            let type_name = self.stack[len - 1].py_type_name(self);
            return Err(ExcType::import_star_name_type(module_name, from_all, &type_name));
        };
        let module = self.stack[len - 3].clone_with_heap(self.heap);
        let this = self;
        defer_drop!(module, this);
        module.py_getattr(&attr, this)
    }

    /// `[name, value] -> []`: binds `name` where a module-level store would,
    /// in the frame's locals dict, its globals dict, or a module global slot.
    pub(super) fn import_star_bind(&mut self) -> RunResult<()> {
        let value = self.pop();
        let name = self.pop();
        let this = self;
        defer_drop!(name, this);
        match this.frame_namespace() {
            (_, Some(dict)) | (FrameGlobals::Dict(dict), None) => {
                let key = name.clone_with_heap(this.heap);
                dict_set(dict, key, value, this)
            }
            (FrameGlobals::Slots, None) => {
                let name = name.as_str(this).expect("import_star_load checked the name is a str");
                let existing = this
                    .interns
                    .get_string_id_by_name(name)
                    .and_then(|name_id| this.global_names.get(name_id));
                // Checked before interning, so a full namespace never grows the interner.
                if existing.is_none() && NamespaceId::new(this.global_names.len()).is_none() {
                    value.drop_with(this);
                    return Err(SimpleException::new_msg(ExcType::SyntaxError, namespace_overflow_message()).into());
                }
                // Code compiled later finds the slot by this ID; a new name takes a
                // fresh slot, as preparing a store to it would.
                let name_id = this.interns.intern_runtime(name);
                let slot = this
                    .global_names
                    .ensure_slot(name_id, CodeRange::default())
                    .expect("slot capacity checked above");
                this.globals.resize_with(this.global_names.len(), || Value::Undefined);
                this.set_global_slot(slot.as_u16(), value);
                Ok(())
            }
        }
    }

    /// The module object `depth` values below the stack top; `ImportError`
    /// for a host value that is not a module, so has no namespace to list.
    fn import_star_module(&self, depth: usize) -> RunResult<&Module> {
        match &self.stack[self.stack.len() - 1 - depth] {
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::Module(module) => Ok(module),
                _ => Err(ExcType::import_star_without_namespace()),
            },
            _ => Err(ExcType::import_star_without_namespace()),
        }
    }
}

/// A module's attribute dict: a file module's namespace, else its `attrs`.
fn module_attrs<'a>(module: &'a Module, vm: &'a VM<'_>) -> RunResult<&'a Dict> {
    match module.namespace() {
        Some(namespace) => match vm.heap.get(namespace) {
            HeapData::Dict(dict) => Ok(dict),
            _ => Err(RunError::internal("module namespace is not a dict")),
        },
        None => Ok(module.attrs()),
    }
}

/// Binds `key` to `value` in the namespace dict `dict_id`, releasing any old value.
fn dict_set(dict_id: HeapId, key: Value, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
    let HeapReadOutput::Dict(dict) = vm.heap.read(dict_id) else {
        key.drop_with(vm);
        value.drop_with(vm);
        return Err(RunError::internal("namespace is not a dict"));
    };
    let old = dict.set(key, value, vm)?;
    old.drop_with(vm);
    Ok(())
}
