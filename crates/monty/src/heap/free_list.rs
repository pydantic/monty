use std::cell::UnsafeCell;

use crate::heap::HeapId;

/// Not-thread-safe free list for `HeapId`s.
pub struct FreeList {
    /// Content of the free list. See `.pop()` and `.push()` for more details.
    ids: UnsafeCell<Vec<HeapId>>,
}

impl FreeList {
    pub fn new() -> Self {
        Self {
            ids: UnsafeCell::new(Vec::new()),
        }
    }

    /// Pops a `HeapId` from the free list. Returns `None` if the free list is empty.
    pub fn pop(&self) -> Option<HeapId> {
        // SAFETY: (DH)
        //   - `.pop()` does not shrink the vector so no reentrancy concerns from an allocator
        //   - `.pop()` does not re-enter into itself, so with `FreeList` being `!Sync`, we can
        //      be sure that there are no concurrent calls to `.pop()` that could cause data races.
        //   - No `FreeList` methods hand out references to the inner vector, `HeapId` is `Copy`,
        //     so the value is trivially copied out without causing other code to run.
        unsafe { (*self.ids.get()).pop() }
    }

    /// Pushes a `HeapId` back to the free list. Borrows the freelist mutably.
    pub fn push(&mut self, id: HeapId) {
        // Requiring `&mut self` avoids unsafe here as well as possible weird edge cases
        // like what if a re-entrant allocator was able to try to push to a static
        // free list while it was already being pushed to.
        self.ids.get_mut().push(id);
    }
}

impl From<Vec<HeapId>> for FreeList {
    fn from(ids: Vec<HeapId>) -> Self {
        Self {
            ids: UnsafeCell::new(ids),
        }
    }
}
