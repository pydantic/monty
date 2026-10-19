//! Borrowed handles must not extend the lifetime or exclusivity of their source borrow.

use super::*;

/// A shared reference cannot escape an externally owned allocation.
#[cfg(heap_reader_compile_fail_test_protected_get_escape)]
fn protected_get_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let source = Box::new(123_u64);
            let protected = heap.protect(&*source);
            let read: &HeapRead<'_, u64> = &protected;
            read.get(heap)
        };
        assert_eq!(*escaped, 123);
    });
}

/// Shared access through an exclusive handle still borrows its source.
#[cfg(heap_reader_compile_fail_test_protected_mut_get_escape)]
fn protected_mut_get_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let mut source = Box::new(123_u64);
            let protected = heap.protect_mut(&mut *source);
            let read: &HeapRead<'_, u64> = &protected;
            read.get(heap)
        };
        assert_eq!(*escaped, 123);
    });
}

/// An exclusive reference cannot escape an externally owned allocation.
#[cfg(heap_reader_compile_fail_test_protected_get_mut_escape)]
fn protected_get_mut_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let mut source = Box::new(123_u64);
            let mut protected = heap.protect_mut(&mut *source);
            let read: &mut HeapRead<'_, u64> = &mut protected;
            read.get_mut(heap)
        };
        *escaped = 456;
    });
}

/// A shared protected reference excludes mutation through the original owner.
#[cfg(heap_reader_compile_fail_test_protected_get_alias)]
fn protected_get_alias(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let mut source = Box::new(123_u64);
        let protected = heap.protect(&*source);
        let borrowed = protected.get(heap);
        *source = 456;
        assert_eq!(*borrowed, 123);
    });
}

/// Shared access through an exclusive handle also excludes source mutation.
#[cfg(heap_reader_compile_fail_test_protected_mut_get_alias)]
fn protected_mut_get_alias(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let mut source = Box::new(123_u64);
        let protected = heap.protect_mut(&mut *source);
        let borrowed = protected.get(heap);
        *source = 456;
        assert_eq!(*borrowed, 123);
    });
}

/// An exclusive protected reference excludes reads through the original owner.
#[cfg(heap_reader_compile_fail_test_protected_get_mut_alias)]
fn protected_get_mut_alias(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let mut source = Box::new(123_u64);
        let mut protected = heap.protect_mut(&mut *source);
        let borrowed = protected.get_mut(heap);
        assert_eq!(*source, 123);
        *borrowed = 456;
    });
}

/// A field projection must preserve the protected source's lifetime.
#[cfg(heap_reader_compile_fail_test_projected_get_escape)]
fn projected_get_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let source = Box::new((123_u64,));
            let protected = heap.protect(&*source);
            let read: &HeapRead<'_, (u64,)> = &protected;
            let field = heap_read_ref_as_field!(read, (u64,), 0);
            field.get(heap)
        };
        assert_eq!(*escaped, 123);
    });
}

/// A slice projection cannot outlive its vector's allocation.
#[cfg(heap_reader_compile_fail_test_slice_get_escape)]
fn slice_get_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let source = vec![123_u64];
            let protected = heap.protect(&source);
            let slice = protected.as_slice(heap);
            slice.get(heap)
        };
        assert_eq!(escaped[0], 123);
    });
}

/// A box projection cannot outlive its box's allocation.
#[cfg(heap_reader_compile_fail_test_box_get_escape)]
fn box_get_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let source = Box::new(123_u64);
            let protected = heap.protect(&source);
            let boxed = protected.as_box_value(heap);
            boxed.get(heap)
        };
        assert_eq!(*escaped, 123);
    });
}
