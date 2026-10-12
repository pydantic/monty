//! Upgrades applied to a loaded heap, so an older dump keeps loading without a
//! `DUMP_VERSION` bump.
//!
//! An upgrade fits a change that alters what stored data *means* while the old
//! data still holds everything needed to rebuild the new form. Bytecode, operand
//! layouts and persisted hashes cannot be rebuilt this way and still need a bump.
//!
//! Rules every upgrade follows:
//! - The dump carries no version to branch on, so old data is recognised by its
//!   shape, or by a `#[serde(default)]` marker that current builds always set
//!   and old data reads back without; current data must be left unchanged.
//! - It runs before any code, as the first VM over a loaded heap is built.
//! - It is tested against a fixture the *older* build wrote, which must never be
//!   regenerated (see `tests/dump_compat.rs`).

use crate::{bytecode::VM, heap::HeapData, modules::dataclasses};

/// Runs every upgrade over the entries a restore recorded (see
/// [`Heap::take_upgrade_candidates`](crate::heap::Heap::take_upgrade_candidates)).
///
/// Called as each VM is built; only the first over a loaded heap finds anything.
pub(crate) fn upgrade_restored(vm: &mut VM<'_>) {
    // A new upgrade adds a branch for each entry type it inspects.
    for id in vm.heap.take_upgrade_candidates() {
        if let HeapData::Class(_) = vm.heap.get(id) {
            dataclasses::upgrade_legacy_class(id, vm);
        }
    }
}
