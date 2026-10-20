//! The sole lifetime-erasure boundary: arena storage and every parked root move together.

use std::{cell::Cell, collections::BTreeMap, mem};

use monty_types::ResourceTracker;

use super::{CcColor, DropWithContext, Heap, HeapEntries, HeapId, HeapIndex, HeapWire, RuntimeRef, restore_entries};
use crate::{bytecode::VMSnapshot, value::Value};

/// A complete interpreter heap, never independently pairable with another owner's roots.
#[derive(Debug)]
pub(crate) struct HeapOwner {
    state: OwnedState<HeapId<'static>>,
}

/// Active access to one owner's storage; the arena brand is independent of reader scopes.
#[derive(Debug)]
pub(crate) struct OwnedState<H: RuntimeRef> {
    pub(crate) heap: Heap<H>,
    pub(crate) globals: Vec<Value<H>>,
    /// Only suspended execution pays for the parked stack and scheduler storage.
    pub(crate) vm_state: Option<Box<VMSnapshot<H>>>,
}

// SAFETY: all pointer-bearing state is private and transferred together. Activation
// borrows this owner exclusively, and no branded value can escape its universal callback.
// The contained interpreter payloads have no thread affinity. Active views remain !Send/!Sync.
unsafe impl Send for HeapOwner {}

impl HeapOwner {
    /// Creates an isolated arena without exposing its erased brand.
    pub(crate) fn new(capacity: usize, tracker: ResourceTracker) -> Self {
        Self {
            state: OwnedState {
                // SAFETY: this heap and its roots are sealed immediately; the erased
                // lifetime is never exposed through any safe owner method.
                heap: unsafe { Heap::new(capacity, tracker) },
                globals: Vec::new(),
                vm_state: None,
            },
        }
    }

    /// Activates all roots together under a fresh invariant brand.
    pub(crate) fn activate<R>(&mut self, f: impl for<'heap> FnOnce(&mut OwnedState<HeapId<'heap>>) -> R) -> R {
        // SAFETY: only lifetimes change, not layout or provenance. The universal callback
        // cannot export its brand, and this exclusive borrow keeps the entire owner alive.
        f(unsafe { mem::transmute::<&mut OwnedState<HeapId<'static>>, &mut OwnedState<HeapId<'_>>>(&mut self.state) })
    }

    /// Inspects parked state without exporting a branded handle or replacing roots.
    pub(crate) fn inspect<R>(&self, f: impl for<'heap> FnOnce(&OwnedState<HeapId<'heap>>) -> R) -> R {
        // SAFETY: as in activate, but shared access cannot replace or mutate any root.
        f(unsafe { mem::transmute::<&OwnedState<HeapId<'static>>, &OwnedState<HeapId<'_>>>(&self.state) })
    }

    /// Resource controls contain no branded state and may be borrowed independently.
    pub(crate) fn tracker(&self) -> &ResourceTracker {
        &self.state.heap.tracker
    }

    /// Updates host-side resource controls without exposing the heap.
    pub(crate) fn tracker_mut(&mut self) -> &mut ResourceTracker {
        &mut self.state.heap.tracker
    }

    /// Reconstructs all roots against reserved destination slots, including forward edges.
    pub(crate) fn from_wire(
        wire: HeapWire,
        globals: Vec<Value<HeapIndex>>,
        vm_state: Option<VMSnapshot<HeapIndex>>,
    ) -> Result<Self, &'static str> {
        // Gray/White would bypass GC marking/scanning that protects live readers.
        // Collection cannot suspend in these states, so even trusted dumps must reject them.
        if wire
            .entries
            .iter()
            .flatten()
            .any(|entry| matches!(entry.color, CcColor::Gray | CcColor::White))
        {
            return Err("snapshot contains a transient GC color");
        }
        // SAFETY: the erased arena is immediately sealed with all relocated roots.
        let mut entries = unsafe { HeapEntries::with_capacity(wire.entries.len()) };
        entries.reserve(wire.entries.len());
        for (index, entry) in wire.entries.into_iter().enumerate() {
            let entry = entry.map(|entry| entry.relocate(&mut |id| entries.id_at(id.index())));
            entries.restore_entry(index, entry);
        }
        let mut reference = |id: HeapIndex| entries.id_at(id.index());
        let globals = globals
            .into_iter()
            .map(|value| value.relocate(&mut reference))
            .collect();
        let vm_state = vm_state.map(|state| Box::new(state.relocate(&mut reference)));
        let timezone_utc = wire.timezone_utc.map(reference);
        let mut owner = Self {
            state: OwnedState {
                heap: Heap {
                    entries,
                    tracker: wire.tracker,
                    purple_count: wire.purple_count,
                    allocations_since_gc: Cell::new(wire.allocations_since_gc),
                    #[cfg(feature = "test-hooks")]
                    gc_disabled: false,
                    timezone_utc,
                    ext_function_cache: BTreeMap::new(),
                    boundary_index: BTreeMap::new(),
                    host_type_index: BTreeMap::new(),
                },
                globals,
                vm_state,
            },
        };
        let indexes = restore_entries(&mut owner.state.heap.entries);
        owner.state.heap.ext_function_cache = indexes.ext_function_cache;
        owner.state.heap.boundary_index = indexes.boundary_index;
        owner.state.heap.host_type_index = indexes.host_type_index;
        Ok(owner)
    }
}

impl Drop for HeapOwner {
    fn drop(&mut self) {
        self.activate(|state| {
            if let Some(snapshot) = state.vm_state.take() {
                let (globals, _, _) = snapshot.abandon(&mut state.heap);
                globals.drop_with(&mut state.heap);
            }
            mem::take(&mut state.globals).drop_with(&mut state.heap);
        });
    }
}
