//! Upgrades applied as `Dump::load` reads a dump older than `DUMP_VERSION`, so
//! `MIN_SUPPORTED_DUMP_VERSION` can stay below it.
//!
//! An upgrade fits a change that alters what stored data *means* while the old
//! data still decodes and holds everything needed to rebuild the new form. A
//! change to one value's stored shape is a `#[serde(from)]` shim on that type
//! instead; bytecode, operand layouts and persisted hashes need the floor raised.
//!
//! Rules every upgrade follows:
//! - It branches on the header version, never on the data's shape, and current
//!   data is never touched.
//! - It runs inside `Dump::load`, before any code: nothing on the live heap or
//!   VM records that a session was upgraded.
//! - It is tested against a fixture the *older* build wrote, which must never be
//!   regenerated (see `tests/dump_compat.rs`).

use crate::{heap::Heap, intern::Interns, modules::dataclasses};

/// Runs every upgrade a dump written at `version` needs over its decoded heap.
pub(crate) fn upgrade(version: u16, heap: &mut Heap, interns: &Interns) {
    if version <= dataclasses::LEGACY_DUMP_VERSION {
        dataclasses::upgrade_legacy_classes(heap, interns);
    }
}
