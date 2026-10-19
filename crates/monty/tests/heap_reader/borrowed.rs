//! Valid borrowed-handle usage, exercised against the production implementation under Miri.

use monty_types::ResourceTracker;

use crate::heap::{Heap, HeapRead, HeapReader};

/// Local external data remains accessible across unrelated heap operations.
#[test]
fn protected_shared_borrow() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    HeapReader::with(&mut heap, &mut (), |heap, ()| {
        let source = Box::new(123_u64);
        let protected = heap.protect(&*source);
        let read: &HeapRead<'_, u64> = &protected;
        assert_eq!(*read.get(heap), 123);
        heap.collect_cycles();
        assert_eq!(*read.get(heap), 123);
    });
}

/// Exclusive handles permit sequential shared and mutable reborrows, then release the source.
#[test]
fn protected_mutable_borrow() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    HeapReader::with(&mut heap, &mut (), |heap, ()| {
        let mut source = Box::new(123_u64);
        {
            let mut protected = heap.protect_mut(&mut *source);
            let read: &mut HeapRead<'_, u64> = &mut protected;
            assert_eq!(*read.get(heap), 123);
            *read.get_mut(heap) = 456;
            heap.collect_cycles();
            assert_eq!(*read.get(heap), 456);
        }
        assert_eq!(*source, 456);
        *source = 789;
        assert_eq!(*source, 789);
    });
}

/// Nested field, slice and box projections remain usable within their source borrow.
#[test]
fn protected_projections() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    HeapReader::with(&mut heap, &mut (), |heap, ()| {
        let mut source = (vec![123_u64], Box::new(456_u64));
        {
            let protected = heap.protect(&source);
            let read: &HeapRead<'_, (Vec<u64>, Box<u64>)> = &protected;
            let field = heap_read_ref_as_field!(read, (Vec<u64>, Box<u64>), 0);
            let slice = field.as_slice(heap);
            let field = heap_read_ref_as_field!(read, (Vec<u64>, Box<u64>), 1);
            let boxed = field.as_box_value(heap);
            heap.collect_cycles();
            assert_eq!(slice.get(heap), &[123]);
            assert_eq!(*boxed.get(heap), 456);
        }
        {
            let mut protected = heap.protect_mut(&mut source);
            let read: &mut HeapRead<'_, (Vec<u64>, Box<u64>)> = &mut protected;
            let mut field = heap_read_ref_as_field_mut!(read, (Vec<u64>, Box<u64>), 0);
            field.get_mut(heap).push(789);
            assert_eq!(field.get(heap), &[123, 789]);
        }
        assert_eq!(source.0, [123, 789]);
    });
}
