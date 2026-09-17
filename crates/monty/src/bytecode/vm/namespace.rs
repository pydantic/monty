//! Frame namespaces: where a frame resolves names that are not stack slots.
//!
//! Ordinary frames resolve every name at compile time to a stack slot or a
//! `VM::globals` slot. Code compiled at runtime by `eval()` / `exec()` cannot:
//! its top level may run against a locals dict, an explicit globals dict, or
//! both, and functions it defines under a globals dict keep resolving their
//! globals through that dict at every call. [`FrameNamespace`] records which
//! case a frame is in; the descriptor owns the dict references it names.

use ahash::AHashSet;

use super::{CallFrame, VM};
use crate::{
    bytecode::{Code, FrameExit, NAME_CALLABLE, NAME_GLOBAL_ONLY},
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::{FunctionId, StringId},
    prepare::SnippetNames,
    types::Dict,
    value::Value,
};

/// Where a frame's global names live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum FrameGlobals {
    /// The module's dense `VM::globals` array, addressed by slot.
    Slots,
    /// An explicit `exec()` / `eval()` globals dict, addressed by name. The
    /// frame owns a reference to it.
    Dict(HeapId),
}

/// The namespaces a frame resolves names through, for the frames that need
/// any: an ordinary frame (locals in stack slots, globals in `VM::globals`)
/// carries `None`, so the common case costs one null pointer.
///
/// Every `HeapId` here is OWNED by the frame: released exactly once by
/// `VM::cleanup_frame_state`, or handed to a serialized frame by
/// `CallFrame::serialize`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum FrameNamespace {
    /// A function or class body defined under an explicit globals dict: locals
    /// in stack slots, globals looked up by name in `globals`.
    Function {
        /// The `exec()` / `eval()` globals dict the function was defined under.
        globals: HeapId,
    },
    /// The top-level frame of an `eval()` / `exec()` snippet: names are looked
    /// up at runtime through `locals` (when present) and then `globals`.
    Snippet {
        /// Where the snippet's globals live.
        globals: FrameGlobals,
        /// The snippet's locals dict, if it runs with one distinct from its globals.
        locals: Option<HeapId>,
    },
}

impl FrameNamespace {
    /// The globals dict a function created in this frame must carry, so its
    /// own frames resolve globals the same way; `None` for slot globals.
    pub(crate) fn dict_globals(&self) -> Option<HeapId> {
        match self {
            Self::Snippet {
                globals: FrameGlobals::Slots,
                ..
            } => None,
            Self::Function { globals }
            | Self::Snippet {
                globals: FrameGlobals::Dict(globals),
                ..
            } => Some(*globals),
        }
    }
}

/// The namespace for a frame of a function that carries `globals`: an owned
/// reference to the dict (inc_ref'd here), or `None` for slot globals.
#[inline]
pub(crate) fn function_namespace(globals: Option<HeapId>, heap: &impl ContainsHeap) -> Option<Box<FrameNamespace>> {
    globals.map(|globals| {
        heap.heap().inc_ref(globals);
        Box::new(FrameNamespace::Function { globals })
    })
}

impl<C: ContainsHeap> DropWithContext<C> for Box<FrameNamespace> {
    fn drop_with(self, ctx: &mut C) {
        match *self {
            FrameNamespace::Function { globals } => ctx.heap_mut().dec_ref(globals),
            FrameNamespace::Snippet { globals, locals } => {
                if let FrameGlobals::Dict(globals) = globals {
                    ctx.heap_mut().dec_ref(globals);
                }
                if let Some(locals) = locals {
                    ctx.heap_mut().dec_ref(locals);
                }
            }
        }
    }
}

impl VM<'_> {
    /// The namespace an `eval()` / `exec()` snippet runs in, from the call's
    /// `globals` / `locals` dicts (owned references, or `None`), and the mode
    /// its top level compiles in.
    ///
    /// Without a `globals` dict the snippet inherits the caller's: a snippet
    /// frame's own namespace (so nested implicit calls share it), a
    /// dict-namespaced function's dict, or the module slots. Without a
    /// `locals` dict a function frame contributes a snapshot of its locals
    /// (PEP 667), a module frame nothing.
    pub(crate) fn snippet_namespace(
        &mut self,
        globals: Option<HeapId>,
        locals: Option<HeapId>,
    ) -> RunResult<(SnippetNames, Box<FrameNamespace>)> {
        let (globals, locals) = match globals {
            Some(dict) => (FrameGlobals::Dict(dict), locals),
            None => match self.current_frame.namespace.as_deref() {
                Some(FrameNamespace::Snippet {
                    globals,
                    locals: frame_locals,
                }) => {
                    let (globals, frame_locals) = (*globals, *frame_locals);
                    if let FrameGlobals::Dict(dict) = globals {
                        self.heap.inc_ref(dict);
                    }
                    let locals = match (locals, frame_locals) {
                        (Some(dict), _) => Some(dict),
                        (None, Some(dict)) => {
                            self.heap.inc_ref(dict);
                            Some(dict)
                        }
                        (None, None) => None,
                    };
                    (globals, locals)
                }
                Some(FrameNamespace::Function { globals }) => {
                    let dict = *globals;
                    let locals = match locals {
                        Some(dict) => Some(dict),
                        None => Some(self.snapshot_locals()?),
                    };
                    self.heap.inc_ref(dict);
                    (FrameGlobals::Dict(dict), locals)
                }
                None => {
                    let locals = match (locals, self.current_frame.function_id) {
                        (Some(dict), _) => Some(dict),
                        (None, Some(_)) => Some(self.snapshot_locals()?),
                        (None, None) => None,
                    };
                    (FrameGlobals::Slots, locals)
                }
            },
        };
        let names = match (globals, locals) {
            (FrameGlobals::Dict(_), _) => SnippetNames::NameOverDict,
            (FrameGlobals::Slots, Some(_)) => SnippetNames::NameOverSlots,
            (FrameGlobals::Slots, None) => SnippetNames::Slots,
        };
        Ok((names, Box::new(FrameNamespace::Snippet { globals, locals })))
    }

    /// Pushes the frame that runs a compiled `eval()` / `exec()` snippet.
    ///
    /// The snippet is a `<module>`-named function with no locals; the frame
    /// takes ownership of `namespace`, and releases it if the push is refused
    /// by the recursion limit.
    pub(crate) fn push_snippet_frame(
        &mut self,
        func_id: FunctionId,
        code: &Code,
        namespace: Box<FrameNamespace>,
    ) -> RunResult<()> {
        let call_offset = self.current_offset();
        let stack_base = self.stack.len();
        let exc_stack_base = self.exception_stack.len();
        self.push_frame(CallFrame::new_function(
            code,
            stack_base,
            0,
            exc_stack_base,
            func_id,
            call_offset,
            Some(namespace),
        ))
    }

    /// The dict `locals()` returns in the current frame.
    ///
    /// A snippet with a locals dict returns that dict itself; one that runs
    /// straight in a globals dict returns the dict. Every other frame gets a
    /// snapshot: named stack slots for a function, bound slots for a module.
    pub(crate) fn locals_dict(&mut self) -> RunResult<Value> {
        let dict_id = match self.current_frame.namespace.as_deref() {
            Some(
                FrameNamespace::Snippet { locals: Some(dict), .. }
                | FrameNamespace::Snippet {
                    globals: FrameGlobals::Dict(dict),
                    locals: None,
                },
            ) => {
                let dict = *dict;
                self.heap.inc_ref(dict);
                dict
            }
            Some(FrameNamespace::Snippet {
                globals: FrameGlobals::Slots,
                locals: None,
            }) => self.snapshot_globals()?,
            Some(FrameNamespace::Function { .. }) => self.snapshot_locals()?,
            None if self.current_frame.function_id.is_none() => self.snapshot_globals()?,
            None => self.snapshot_locals()?,
        };
        Ok(Value::Ref(dict_id))
    }

    /// A fresh dict of the bound module globals, for `locals()` at module scope.
    fn snapshot_globals(&mut self) -> RunResult<HeapId> {
        let dict_id = self.heap.allocate(HeapData::Dict(Dict::new()));
        // Owned by the guard until every entry is in, so a failed insert frees it.
        let mut dict_guard = DropGuard::new(Value::Ref(dict_id), self);
        let (_, this) = dict_guard.as_parts_mut();
        for slot in 0..this.global_names.len() {
            if !matches!(this.globals[slot], Value::Undefined) {
                let name_id = this.global_names.names()[slot];
                let value = this.globals[slot].clone_with_heap(this.heap);
                this.namespace_set(dict_id, name_id, value)?;
            }
        }
        let (dict, _) = dict_guard.into_parts();
        Ok(dict.into_ref_id().expect("snapshot dict is a heap reference"))
    }

    /// `LoadName`: resolves `name_id` through the frame's namespace and pushes it.
    ///
    /// With slot globals the tail of the lookup is [`load_global`](Self::load_global)
    /// (or the callable variant under `NAME_CALLABLE`), so builtins, module
    /// dunders and the host `NameLookup` suspension behave exactly as for
    /// compiled code. Dict globals see the dict and builtins only: CPython
    /// resolves nothing else for `exec(src, {})`.
    pub(super) fn load_name(&mut self, slot: u16, name_id: StringId, flags: u8) -> RunResult<Option<FrameExit>> {
        let (globals, locals) = self.frame_namespace();
        if flags & NAME_GLOBAL_ONLY == 0
            && let Some(locals) = locals
            && let Some(value) = self.namespace_get(locals, name_id)?
        {
            self.push(value);
            return Ok(None);
        }
        match globals {
            FrameGlobals::Slots if flags & NAME_CALLABLE != 0 => {
                self.load_global_callable(slot, name_id);
                Ok(None)
            }
            FrameGlobals::Slots => self.load_global(slot),
            FrameGlobals::Dict(dict) => {
                if let Some(value) = self.namespace_get(dict, name_id)? {
                    self.push(value);
                } else if let Some(builtin) = self.builtin_for_name(name_id) {
                    self.push(builtin);
                } else {
                    return Err(ExcType::name_error(self.interns.get_str(name_id)).into());
                }
                Ok(None)
            }
        }
    }

    /// `StoreName`: pops the value and binds `name_id` in the locals dict when
    /// there is one (and `NAME_GLOBAL_ONLY` is clear), else in the globals.
    pub(super) fn store_name(&mut self, slot: u16, name_id: StringId, flags: u8) -> RunResult<()> {
        let value = self.pop();
        match self.frame_namespace() {
            (_, Some(locals)) if flags & NAME_GLOBAL_ONLY == 0 => self.namespace_set(locals, name_id, value),
            (FrameGlobals::Slots, _) => {
                self.set_global_slot(slot, value);
                Ok(())
            }
            (FrameGlobals::Dict(dict), _) => self.namespace_set(dict, name_id, value),
        }
    }

    /// `DeleteName`: unbinds `name_id` where [`store_name`](Self::store_name)
    /// would bind it; `NameError` if it is not bound there.
    pub(super) fn delete_name(&mut self, slot: u16, name_id: StringId, flags: u8) -> RunResult<()> {
        let removed = match self.frame_namespace() {
            (_, Some(locals)) if flags & NAME_GLOBAL_ONLY == 0 => self.namespace_pop(locals, name_id)?,
            (FrameGlobals::Slots, _) => return self.delete_global(slot),
            (FrameGlobals::Dict(dict), _) => self.namespace_pop(dict, name_id)?,
        };
        if removed {
            Ok(())
        } else {
            Err(ExcType::name_error(self.interns.get_str(name_id)).into())
        }
    }

    /// Builds the dict `locals()` reports for the current frame: a snapshot of
    /// its named stack slots, with captured variables read through their cells
    /// (PEP 667 semantics — writes to the dict never reach the frame).
    ///
    /// Returns an owned reference to the new dict.
    pub(crate) fn snapshot_locals(&mut self) -> RunResult<HeapId> {
        let dict_id = self.heap.allocate(HeapData::Dict(Dict::new()));
        // Owned by the guard until every entry is in, so a failed insert frees it.
        let mut dict_guard = DropGuard::new(Value::Ref(dict_id), self);
        let (_, this) = dict_guard.as_parts_mut();
        let base = this.current_frame.stack_base();
        let code = this.frame_code(&this.current_frame);
        let names = (0..this.current_frame.locals_count).filter_map(|slot| {
            code.local_name(slot)
                .filter(|name_id| *name_id != StringId::default())
                .map(|name_id| (usize::from(slot), name_id))
        });
        // Cell slots go last so a captured parameter's live cell value replaces
        // the stale copy left in its parameter slot under the same name.
        let cell_slots: AHashSet<usize> = this
            .current_frame
            .function_id
            .map(|id| this.interns.get_function(id))
            .into_iter()
            .flat_map(|func| {
                func.cell_var_slots
                    .iter()
                    .chain(&func.free_var_slots)
                    .map(|slot| slot.index())
            })
            .collect();
        for (slot, name_id) in names.clone().filter(|(slot, _)| !cell_slots.contains(slot)) {
            let value = this.stack[base + slot].clone_with_heap(this.heap);
            this.snapshot_entry(dict_id, name_id, value)?;
        }
        for (slot, name_id) in names.filter(|(slot, _)| cell_slots.contains(slot)) {
            let value = match &this.stack[base + slot] {
                Value::Ref(cell_id) => match this.heap.get(*cell_id) {
                    HeapData::Cell(cell) => cell.0.clone_with_heap(this.heap),
                    _ => Value::Undefined,
                },
                _ => Value::Undefined,
            };
            this.snapshot_entry(dict_id, name_id, value)?;
        }
        let (dict, _) = dict_guard.into_parts();
        Ok(dict.into_ref_id().expect("snapshot dict is a heap reference"))
    }

    /// Stores one `locals()` entry, skipping unbound slots.
    fn snapshot_entry(&mut self, dict_id: HeapId, name_id: StringId, value: Value) -> RunResult<()> {
        if matches!(value, Value::Undefined) {
            Ok(())
        } else {
            self.namespace_set(dict_id, name_id, value)
        }
    }

    /// The globals and locals the current frame resolves names through.
    ///
    /// A frame with no namespace resolves like compiled module code.
    fn frame_namespace(&self) -> (FrameGlobals, Option<HeapId>) {
        match self.current_frame.namespace.as_deref() {
            None => (FrameGlobals::Slots, None),
            Some(FrameNamespace::Function { globals }) => (FrameGlobals::Dict(*globals), None),
            Some(FrameNamespace::Snippet { globals, locals }) => (*globals, *locals),
        }
    }

    /// Looks `name_id` up in a namespace dict, returning an owned value.
    fn namespace_get(&mut self, dict_id: HeapId, name_id: StringId) -> RunResult<Option<Value>> {
        let HeapReadOutput::Dict(dict) = self.heap.read(dict_id) else {
            return Err(RunError::internal("namespace is not a dict"));
        };
        dict.dict_get(&Value::InternString(name_id), self)
    }

    /// Binds `name_id` to `value` in a namespace dict, releasing any old value.
    fn namespace_set(&mut self, dict_id: HeapId, name_id: StringId, value: Value) -> RunResult<()> {
        let HeapReadOutput::Dict(mut dict) = self.heap.read(dict_id) else {
            value.drop_with(self);
            return Err(RunError::internal("namespace is not a dict"));
        };
        let old = dict.set(Value::InternString(name_id), value, self)?;
        old.drop_with(self);
        Ok(())
    }

    /// Removes `name_id` from a namespace dict; `false` if it was not bound.
    fn namespace_pop(&mut self, dict_id: HeapId, name_id: StringId) -> RunResult<bool> {
        let HeapReadOutput::Dict(mut dict) = self.heap.read(dict_id) else {
            return Err(RunError::internal("namespace is not a dict"));
        };
        match dict.pop(&Value::InternString(name_id), self)? {
            Some((key, value)) => {
                key.drop_with(self);
                value.drop_with(self);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
