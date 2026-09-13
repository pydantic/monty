//! Frame namespaces: where a frame resolves names that are not stack slots.
//!
//! Ordinary frames resolve every name at compile time to a stack slot or a
//! `VM::globals` slot. Code compiled at runtime by `eval()` / `exec()` cannot:
//! its top level may run against a locals dict, an explicit globals dict, or
//! both, and functions it defines under a globals dict keep resolving their
//! globals through that dict at every call. [`FrameNamespace`] records which
//! case a frame is in; the descriptor owns the dict references it names.

use crate::heap::{ContainsHeap, DropWithContext, HeapId};

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
