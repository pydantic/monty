//! Valid borrowed-handle usage, exercised against the production implementation under Miri.

use monty_types::ResourceTracker;

use crate::heap::{Heap, HeapRead, HeapReadMut, HeapReader, HeapView};

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

/// Aliases of a mutable capability serialize payload access through the reader.
#[test]
fn protected_mutable_borrow() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    HeapReader::with(&mut heap, &mut (), |heap, ()| {
        let mut source = Box::new(123_u64);
        {
            let protected = heap.protect_mut(&mut *source);
            let first: &HeapReadMut<'_, u64> = &protected;
            let second = first;
            assert_eq!(*first.get(heap), 123);
            *second.get_mut(heap) = 456;
            heap.collect_cycles();
            assert_eq!(*first.get(heap), 456);
            *first.get_mut(heap) = 789;
            assert_eq!(*second.get(heap), 789);
        }
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
            let slice = field.as_slice();
            let field = heap_read_ref_as_field!(read, (Vec<u64>, Box<u64>), 1);
            let boxed = field.as_box_value();
            heap.collect_cycles();
            assert_eq!(slice.get(heap), &[123]);
            assert_eq!(*boxed.get(heap), 456);
        }
        {
            let protected = heap.protect_mut(&mut source);
            let read: &HeapReadMut<'_, (Vec<u64>, Box<u64>)> = &protected;
            let field = heap_read_ref_as_field_mut!(read, (Vec<u64>, Box<u64>), 0);
            field.get_mut(heap).push(789);
            assert_eq!(field.get(heap), &[123, 789]);
        }
        assert_eq!(source.0, [123, 789]);
    });
}

/// Projections resolve fresh allocations after mutation through an aliased parent.
#[test]
fn projections_follow_replacement() {
    let mut heap = Heap::new(16, ResourceTracker::default());
    HeapReader::with(&mut heap, &mut (), |heap, ()| {
        let mut source = (vec![123_u64], Box::new(456_u64));
        let protected = heap.protect_mut(&mut source);
        let read: &HeapReadMut<'_, (Vec<u64>, Box<u64>)> = &protected;
        let vector = heap_read_ref_as_field_mut!(read, (Vec<u64>, Box<u64>), 0);
        let slice = vector.as_slice();
        let boxed_field = heap_read_ref_as_field!(read, (Vec<u64>, Box<u64>), 1);
        let boxed = boxed_field.as_box_value();
        assert_eq!(slice.get(heap), &[123]);
        assert_eq!(*boxed.get(heap), 456);

        *read.get_mut(heap) = (vec![789, 1011], Box::new(1213));
        heap.collect_cycles();
        assert_eq!(slice.get(heap), &[789, 1011]);
        assert_eq!(*boxed.get(heap), 1213);

        *vector.get_mut(heap) = vec![1415];
        assert_eq!(slice.get(heap), &[1415]);
        vector.get_mut(heap).resize(1024, 1617);
        assert_eq!(slice.get(heap).len(), 1024);
        assert_eq!(slice.get(heap)[1023], 1617);
    });
}
