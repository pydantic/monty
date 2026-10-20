//! Borrowed handles must not extend the lifetime or exclusivity of their source borrow.

use super::*;
#[cfg(heap_reader_compile_fail_test_counted_inner_swap)]
use crate::types::List;

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
            let protected = heap.protect_mut(&mut *source);
            let read: &HeapReadMut<'_, u64> = &protected;
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
        let protected = heap.protect_mut(&mut *source);
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
            let slice = protected.as_slice();
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
            let boxed = protected.as_box_value();
            boxed.get(heap)
        };
        assert_eq!(*escaped, 123);
    });
}

/// Mutable capabilities cannot be exchanged independently of their source borrows.
#[cfg(heap_reader_compile_fail_test_mutable_inner_swap)]
#[expect(unused_mut, reason = "the rejected DerefMut operations would require mutable owners")]
fn mutable_inner_swap(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let mut source = 123_u64;
        let mut outer = heap.protect_mut(&mut source);
        {
            let mut source = Box::new(456_u64);
            let mut inner = heap.protect_mut(&mut *source);
            std::mem::swap(&mut *outer, &mut *inner);
        }
        assert_eq!(*outer.get(heap), 123);
    });
}

/// Read-only capabilities also cannot be detached from their source borrows.
#[cfg(heap_reader_compile_fail_test_shared_inner_swap)]
#[expect(unused_mut, reason = "the rejected DerefMut operations would require mutable owners")]
fn shared_inner_swap(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let source = 123_u64;
        let mut outer = heap.protect(&source);
        {
            let source = Box::new(456_u64);
            let mut inner = heap.protect(&*source);
            std::mem::swap(&mut *outer, &mut *inner);
        }
        assert_eq!(*outer.get(heap), 123);
    });
}

/// An object's capability cannot be exchanged with an uncounted borrowed capability.
#[cfg(heap_reader_compile_fail_test_counted_inner_swap)]
#[expect(unused_mut, reason = "the rejected DerefMut operations would require mutable owners")]
fn counted_inner_swap(list_id: HeapId, heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let HeapReadOutput::List(mut object) = heap.read(list_id) else {
            unreachable!()
        };
        let mut source = List::new(Vec::new());
        let mut borrowed = heap.protect_mut(&mut source);
        std::mem::swap(&mut *object, &mut *borrowed);
    });
}

/// A shared source borrow never authorizes payload mutation.
#[cfg(heap_reader_compile_fail_test_readonly_mutation)]
fn readonly_mutation(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let source = 123_u64;
        let protected = heap.protect(&source);
        *protected.get_mut(heap) = 456;
    });
}

/// A read-only capability cannot be upgraded by projecting a mutable field.
#[cfg(heap_reader_compile_fail_test_readonly_mutable_projection)]
fn readonly_mutable_projection(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let source = (123_u64,);
        let protected = heap.protect(&source);
        let read: &HeapRead<'_, (u64,)> = &protected;
        let field = heap_read_ref_as_field_mut!(read, (u64,), 0);
        *field.get_mut(heap) = 456;
    });
}

/// Aliased capabilities still cannot create overlapping mutable and shared payload borrows.
#[cfg(heap_reader_compile_fail_test_aliased_payload_borrows)]
fn aliased_payload_borrows(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let mut source = 123_u64;
        let protected = heap.protect_mut(&mut source);
        let first: &HeapReadMut<'_, u64> = &protected;
        let second = first;
        let mutable = first.get_mut(heap);
        assert_eq!(*second.get(heap), *mutable);
    });
}

/// Mutable field references retain the source borrow through the projection.
#[cfg(heap_reader_compile_fail_test_projected_get_mut_escape)]
fn projected_get_mut_escape(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let escaped = {
            let mut source = (123_u64,);
            let protected = heap.protect_mut(&mut source);
            let read: &HeapReadMut<'_, (u64,)> = &protected;
            let field = heap_read_ref_as_field_mut!(read, (u64,), 0);
            field.get_mut(heap)
        };
        *escaped = 456;
    });
}

/// Replacing a vector is blocked while a projected slice reference is live.
#[cfg(heap_reader_compile_fail_test_projection_live_borrow)]
fn projection_live_borrow(heap: &mut Heap) {
    HeapReader::with(heap, &mut (), |heap, ()| {
        let mut source = vec![123_u64];
        let protected = heap.protect_mut(&mut source);
        let slice = protected.as_slice();
        let borrowed = slice.get(heap);
        *protected.get_mut(heap) = vec![456];
        assert_eq!(borrowed, &[123]);
    });
}
