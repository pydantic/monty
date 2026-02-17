//! Implementation of the next() builtin function.

use crate::{
    args::ArgValues, defer_drop, exception_private::RunResult, heap::Heap, intern::Interns, resource::ResourceTracker,
    types::iter::iterator_next, value::Value,
};

/// Implementation of the next() builtin function.
///
/// Retrieves the next item from an iterator.
///
/// Two forms are supported:
/// - `next(iterator)` - Returns the next item from the iterator. Raises
///   `StopIteration` when the iterator is exhausted.
/// - `next(iterator, default)` - Returns the next item from the iterator, or
///   `default` if the iterator is exhausted.
pub fn builtin_next(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (iterator, default) = args.get_one_two_args("next", heap)?;
    defer_drop!(iterator, heap);
    iterator_next(iterator, default, heap, interns)
}
