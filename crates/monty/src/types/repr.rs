//! Item-formatting helpers shared by container reprs.
//!
//! List, tuple, deque, set, dict, `functools.partial`, generic-alias, union and dataclass reprs
//! use these, so they write items and poll the resource limits between them the same way.

use std::fmt::Write;

use monty_types::ResourceError;

use crate::{
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::HeapReader,
    types::{LazyHeapSet, PyTrait},
    value::Value,
};

/// Writes a formatted sequence of values to a formatter.
///
/// This helper function is used to implement `__repr__` for sequence types like
/// lists and tuples. It writes items as comma-separated repr interns.
///
/// There is no length parameter: `get_item` is asked for indices from 0 until
/// it returns `None`. For an immutable sequence (tuple) that is simply its
/// fixed end; for a mutable one (list) it means a user `__repr__` growing or
/// shrinking the sequence mid-format is observed rather than panicking on a
/// stale bound — CPython's list repr re-checks the live length the same way.
///
/// # Arguments
/// * `start` - The opening character (e.g., '[' for lists, '(' for tuples)
/// * `end` - The closing character (e.g., ']' for lists, ')' for tuples)
/// * `get_item` - Returns an owned clone of the i-th value via brief heap access,
///   or `None` once `i` is out of range. The clone keeps the item alive across
///   user `__repr__` calls that may remove it from the container.
/// * `f` - The formatter to write to
/// * `vm` - The VM for resolving value references and looking up interned strings
/// * `heap_ids` - Set of heap IDs being repr'd (for cycle detection)
pub(crate) fn repr_sequence_fmt<'h>(
    start: char,
    end: char,
    get_item: impl Fn(&HeapReader<'h>, usize) -> Option<Value>,
    f: &mut impl Write,
    vm: &mut VM<'h>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    // Check depth limit before recursing
    let Ok(mut guard) = vm.recursion_guard() else {
        return Ok(f.write_str("...")?);
    };
    let vm = &mut *guard;

    f.write_char(start)?;
    // The item is fetched before the separator is written so nothing is emitted
    // for an index that turned out to be out of range. `repr_should_truncate` bounds
    // reprs that keep growing the sequence from inside a user `__repr__`.
    for i in 0.. {
        let Some(item) = get_item(vm.heap, i) else { break };
        defer_drop!(item, vm);
        if i > 0 {
            if repr_should_truncate(i, vm)? {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        item.py_repr_fmt(f, vm, heap_ids)?;
    }
    f.write_char(end)?;

    Ok(())
}

/// Writes the comma-separated reprs of an already-snapshotted item slice.
///
/// The slice is a refcount-bumped snapshot owned by the caller (deque, set),
/// so items are formatted by reference with no per-item clone; the caller owns
/// the recursion guard and any surrounding brackets. Live containers must go
/// through [`repr_sequence_fmt`] instead, which re-fetches (and clones) each
/// item so a mutating user `__repr__` is observed safely.
pub(crate) fn repr_items_fmt(
    items: &[Value],
    f: &mut impl Write,
    vm: &mut VM<'_>,
    heap_ids: &mut LazyHeapSet,
) -> RunResult<()> {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            if repr_should_truncate(i, vm)? {
                f.write_str(", ...[timeout]")?;
                break;
            }
            f.write_str(", ")?;
        }
        item.py_repr_fmt(f, vm, heap_ids)?;
    }
    Ok(())
}

/// Polls the time and memory soft limits every 64th item of a container repr.
///
/// `Ok(true)` means the time limit ran out, so the caller writes `", ...[timeout]"`
/// and stops (tested in `resource_limits.rs`). A memory error propagates instead,
/// since a truncated repr would let the feed carry on past `max_memory`.
pub(crate) fn repr_should_truncate(i: usize, vm: &VM<'_>) -> RunResult<bool> {
    match vm.heap.tracker.check_memory_time_every(i) {
        Ok(()) => Ok(false),
        Err(ResourceError::Time { .. }) => Ok(true),
        Err(err) => Err(err.into()),
    }
}
