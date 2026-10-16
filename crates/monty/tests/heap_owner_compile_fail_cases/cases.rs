//! Compile-time checks of the production owner's generative boundary.

use std::{mem, thread};

use monty_types::ResourceTracker;
#[cfg(any(
    heap_owner_compile_fail = "deserialize_pointer",
    heap_owner_compile_fail = "deserialize_heap"
))]
use serde::{Deserialize, Deserializer};

use crate::{
    builtins::Builtins,
    bytecode::VMSnapshot,
    heap::{Heap, HeapEntry, HeapId, HeapIndex, HeapOwner, HeapReadOutput, HeapReader, RuntimeRef},
    heap_data::HeapData,
    types::{List, Type},
    value::{NoHeapRef, Value},
};

/// Checks capabilities and ordinary operations before any negative case is enabled.
#[expect(dead_code, reason = "compile-pass control exercised by the UI harness")]
fn control() {
    assert_send::<HeapOwner>();
    assert_send::<HeapEntry<HeapIndex>>();
    assert_send::<VMSnapshot<HeapIndex>>();
    assert_send::<ResourceTracker>();
    assert_eq!(size_of::<HeapId<'_>>(), size_of::<usize>());
    let _constant: Value<NoHeapRef> = Value::Builtin(Builtins::Type(Type::Int));
    let mut owner = HeapOwner::new(0, ResourceTracker::default());
    owner.activate(|state| {
        let heap: &Heap<_> = &state.heap;
        let id = heap.allocate(HeapData::List(List::new(Vec::new())));
        let _ = id.index();
        HeapReader::with(&mut state.heap, &mut (), |heap, ()| {
            let HeapReadOutput::List(list) = heap.read(id) else {
                unreachable!()
            };
            let payload = list.get(heap);
            let HeapReadOutput::List(second) = heap.read(id) else {
                unreachable!()
            };
            assert_eq!(payload.as_slice().len(), second.get(heap).as_slice().len());
        });
        Value::Ref(id).drop_with(&mut state.heap);
    });
    let mut other = HeapOwner::new(0, ResourceTracker::default());
    mem::swap(&mut owner, &mut other);
    thread::spawn(move || owner).join().unwrap();
}

/// Wire payloads must introduce no thread-affine fields hidden by the owner's Send implementation.
fn assert_send<T: Send>() {}

/// An arena pointer cannot leave its activation, even when the owner remains alive.
#[cfg(heap_owner_compile_fail = "escape_pointer")]
fn escape_pointer(owner: &mut HeapOwner) {
    let _pointer = owner.activate(|state| state.heap.allocate(HeapData::List(List::new(Vec::new()))));
}

/// Retaining an owned value between activations must go through the owner's root storage.
#[cfg(heap_owner_compile_fail = "retain_root")]
fn retain_root(owner: &mut HeapOwner) {
    let mut root = None;
    owner.activate(|state| root = Some(Value::Ref(state.heap.allocate(HeapData::List(List::new(Vec::new()))))));
    drop(root);
}

/// A second owner cannot read pointers minted by the first.
#[cfg(heap_owner_compile_fail = "foreign_read")]
fn foreign_read(left: &mut HeapOwner, right: &mut HeapOwner) {
    left.activate(|left| {
        let id = left.heap.allocate(HeapData::List(List::new(Vec::new())));
        right.activate(|right| {
            right.heap.get(id);
        });
    });
}

/// Stashing a foreign pointer as an idle root would make owner destruction unsound.
#[cfg(heap_owner_compile_fail = "foreign_root")]
fn foreign_root(left: &mut HeapOwner, right: &mut HeapOwner) {
    left.activate(|left| {
        let id = left.heap.allocate(HeapData::List(List::new(Vec::new())));
        right.activate(|right| right.globals.push(Value::Ref(id)));
    });
}

/// Complete owners may move, but their arena fields cannot be independently exchanged.
#[cfg(heap_owner_compile_fail = "swap_heaps")]
fn swap_heaps(left: &mut HeapOwner, right: &mut HeapOwner) {
    left.activate(|left| right.activate(|right| mem::swap(&mut left.heap, &mut right.heap)));
}

/// Replacing complete active states is also forbidden across independent brands.
#[cfg(heap_owner_compile_fail = "swap_states")]
fn swap_states(left: &mut HeapOwner, right: &mut HeapOwner) {
    left.activate(|left| right.activate(|right| mem::swap(left, right)));
}

/// No safe constructor can manufacture an independently replaceable arena with a chosen brand.
#[cfg(heap_owner_compile_fail = "manufacture_heap")]
fn manufacture_heap() {
    let _heap: Heap<HeapId<'static>> = Heap::new(0, ResourceTracker::default());
}

/// Pointer decoding is available only as part of reconstructing a complete owner.
#[cfg(heap_owner_compile_fail = "deserialize_pointer")]
fn deserialize_pointer<'de>(decoder: impl Deserializer<'de>) {
    let _pointer: HeapId<'de> = Deserialize::deserialize(decoder).unwrap();
}

/// Standalone arena deserialization must not reintroduce caller-selected brands.
#[cfg(heap_owner_compile_fail = "deserialize_heap")]
fn deserialize_heap<'de>(decoder: impl Deserializer<'de>) {
    let _heap: Heap<HeapId<'de>> = Deserialize::deserialize(decoder).unwrap();
}

/// Constants reject indirect instance-type references as well as direct Value::Ref pointers.
#[cfg(heap_owner_compile_fail = "indirect_constant")]
fn indirect_constant(id: HeapId<'_>) -> Value<NoHeapRef> {
    Value::Builtin(Builtins::Type(Type::Instance(id)))
}

/// Scoped threads cannot transfer an active pointer even without requiring a static lifetime.
#[cfg(heap_owner_compile_fail = "send_pointer")]
fn send_pointer(owner: &mut HeapOwner) {
    owner.activate(|state| {
        let id = state.heap.allocate(HeapData::List(List::new(Vec::new())));
        thread::scope(|scope| {
            scope.spawn(move || id.index());
        });
    });
}

/// Active mutable views cannot be transferred separately from their owner.
#[cfg(heap_owner_compile_fail = "send_view")]
fn send_view(owner: &mut HeapOwner) {
    owner.activate(|state| {
        thread::scope(|scope| {
            scope.spawn(move || state.heap.collect_cycles());
        })
    });
}

/// A shared owner is not a synchronization primitive for interior-mutable payloads.
#[cfg(heap_owner_compile_fail = "sync_owner")]
fn sync_owner() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<HeapOwner>();
}
