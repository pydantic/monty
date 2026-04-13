//! Compile-fail soundness tests for the `HeapReader` API.
//!
//! Each function here exercises a pattern that MUST be rejected by the borrow checker.
//! They are gated behind individual `cfg` flags so the integration test harness
//! (`tests/heap_reader_compile_fail.rs`) can compile each one independently and assert
//! that it fails with the expected error.
//!
//! These tests are never compiled during normal builds — only when the
//! `heap_reader_compile_fail_tests` cfg (plus a per-test cfg) is set.

use super::*;

/// Must not compile: allocating on the heap while holding a reference derived from `HeapRead::get`.
///
/// `a.get(heap)` borrows `heap` immutably, and the resulting slice keeps that borrow alive.
/// `heap.heap_mut().allocate(...)` requires mutable access to `heap`, which
/// conflicts with the live immutable borrow.
///
/// Expected: E0502 (cannot borrow `*heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_heap_mutation_while_reading)]
fn heap_mutation_while_reading(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, |heap| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let slice = a.get(heap).as_slice();
        let _ = heap.heap_mut().allocate(HeapData::Str(Str::new("boom".into())));
        let _ = slice.len();
    });
}

/// Must not compile: two simultaneous `get_mut` calls produce aliasing `&mut` references.
///
/// `get_mut` requires `&mut HeapReader`, so a second `get_mut` while the first's return
/// value is still live creates a double mutable borrow.
///
/// Expected: E0499 (cannot borrow `*heap` as mutable more than once at a time)
#[cfg(heap_reader_compile_fail_test_double_get_mut)]
fn double_get_mut(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, |heap| {
        let mut a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let mut b = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let ref_a = a.get_mut(heap);
        let ref_b = b.get_mut(heap);
        let _ = (ref_a, ref_b);
    });
}

/// Must not compile: calling `dec_ref` while holding a `HeapRead`-derived reference.
///
/// `dec_ref` can free the entry (setting the slot to `None` and dropping the `HeapValue`),
/// which would leave the reference from `get()` dangling.
///
/// Expected: E0502 (cannot borrow `*heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_dec_ref_while_reading)]
fn dec_ref_while_reading(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, |heap| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let list_ref = a.get(heap);
        heap.heap_mut().dec_ref(list_id);
        let _ = list_ref.as_slice().len();
    });
}

/// Must not compile: smuggling a `HeapRead` out of the `HeapReader::with` closure.
///
/// The `for<'a>` bound on `HeapReader::with` means `'a` is universally quantified,
/// so `HeapRead<'a, _>` cannot outlive the closure.
///
/// Expected: E0521 (borrowed data escapes outside of closure)
#[cfg(heap_reader_compile_fail_test_smuggle_heap_read)]
fn smuggle_heap_read(list_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    let mut smuggled: Option<HeapRead<'_, List>> = None;
    HeapReader::with(heap, |heap| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        smuggled = Some(a);
    });
    let _ = smuggled;
}

/// Must not compile: heap mutation inside a `.map()` closure while iterating over
/// data derived from `HeapRead::get`.
///
/// The iterator borrows the slice (which keeps `heap` immutably borrowed), and the
/// closure attempts to capture `heap` for mutable access.
///
/// Expected: E0500 (closure requires unique access to `*heap` but it is already borrowed)
#[cfg(heap_reader_compile_fail_test_mutation_in_map_closure)]
fn mutation_in_map_closure(list_id: HeapId, other_id: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, |heap| {
        let a = match heap.read(list_id) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let result: Vec<bool> = a
            .get(heap)
            .as_slice()
            .iter()
            .map(|_v| {
                heap.heap_mut().dec_ref(other_id);
                true
            })
            .collect();
        let _ = result;
    });
}

/// Must not compile: calling `read()` (which takes `&mut self`) while holding a live
/// reference from `get()` (which borrows `self` as `&`).
///
/// Expected: E0502 (cannot borrow `*heap` as mutable because it is also borrowed as immutable)
#[cfg(heap_reader_compile_fail_test_read_while_ref_alive)]
fn read_while_ref_alive(id_a: HeapId, id_b: HeapId, heap: &mut Heap<impl ResourceTracker>) {
    HeapReader::with(heap, |heap| {
        let a = match heap.read(id_a) {
            HeapReadOutput::List(list) => list,
            _ => unreachable!(),
        };
        let a_ref = a.get(heap);
        let _b = heap.read(id_b);
        let _ = a_ref.as_slice();
    });
}
