//! Runtime checks for the sealed owner and its permanent pointer slots.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    thread,
};

use monty_types::ResourceTracker;

use crate::{
    heap::{ContainsHeap, HeapOwner, HeapReadOutput, HeapReader, RuntimeRef},
    heap_data::HeapData,
    types::List,
    value::Value,
};

/// Shared payload borrows survive slot reuse, new pages and growth of the page directory.
#[test]
fn allocation_preserves_live_payload_borrows() {
    let mut owner = populated_owner();
    owner.activate(|state| {
        let Value::Ref(id) = state.globals[0] else {
            unreachable!()
        };
        Value::Ref(state.heap.allocate(HeapData::List(List::new(Vec::new())))).drop_with(&mut state.heap);
        HeapReader::with(&mut state.heap, &mut state.globals, |heap, globals| {
            let HeapReadOutput::List(list) = heap.read(id) else {
                unreachable!()
            };
            let payload = list.get(heap);
            for i in 0..1300 {
                globals.push(Value::Ref(
                    heap.heap().allocate(HeapData::List(List::new(vec![Value::Int(i)]))),
                ));
            }
            assert!(matches!(payload.as_slice(), [Value::Int(42)]));
        });
    });
}

/// Unwinding an activation releases readers without losing the owner's parked roots.
#[test]
fn activation_unwinds_before_reactivation() {
    let mut owner = populated_owner();
    let result = catch_unwind(AssertUnwindSafe(|| {
        owner.activate(|state| {
            let Value::Ref(id) = state.globals[0] else {
                unreachable!()
            };
            HeapReader::with(&mut state.heap, &mut (), |heap, ()| {
                let _read = heap.read(id);
                panic!("abort activation");
            });
        });
    }));
    assert!(result.is_err());
    owner.activate(|state| {
        let value = state.globals.pop().unwrap();
        value.drop_with(&mut state.heap);
        state.heap.collect_cycles();
    });
}

/// Moving the complete owner between threads preserves its internal addresses and roots.
#[test]
fn complete_owner_moves_between_threads() {
    let owner = populated_owner();
    let mut owner = thread::spawn(move || {
        owner.inspect(|state| assert_eq!(state.globals.len(), 1));
        owner
    })
    .join()
    .unwrap();
    owner.activate(|state| {
        let Value::Ref(id) = state.globals[0] else {
            unreachable!()
        };
        HeapReader::with(&mut state.heap, &mut (), |heap, ()| {
            let HeapReadOutput::List(list) = heap.read(id) else {
                unreachable!()
            };
            assert!(matches!(list.get(heap).as_slice(), [Value::Int(42)]));
        });
    });
}

/// Non-owning handles retain numeric identity after payload destruction and slot reuse.
#[test]
fn vacant_slot_identity_survives_reuse() {
    let mut owner = populated_owner();
    owner.activate(|state| {
        let id = state.heap.allocate(HeapData::List(List::new(Vec::new())));
        let index = id.index();
        Value::Ref(id).drop_with(&mut state.heap);
        assert_eq!(id.index(), index);
        let replacement = state.heap.allocate(HeapData::List(List::new(Vec::new())));
        state.globals.push(Value::Ref(replacement));
        assert_eq!(replacement.index(), index);
        assert_eq!(id.index(), index);
    });
}

/// Creates an owner with an owned root, rather than returning independently pairable storage.
fn populated_owner() -> HeapOwner {
    let mut owner = HeapOwner::new(0, ResourceTracker::default());
    owner.activate(|state| {
        state.globals.push(Value::Ref(
            state.heap.allocate(HeapData::List(List::new(vec![Value::Int(42)]))),
        ));
    });
    owner
}
