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

use ahash::AHashMap;
use ruff_python_parser::parse_module;

use crate::{
    heap::Heap,
    intern::{Interns, StringId},
    modules::dataclasses,
    parse::CodeRange,
};

/// The last dump version whose `eval()` positions count from after the source's
/// leading whitespace, which those builds stripped with `str::trim_start`.
const LEGACY_EVAL_OFFSETS_VERSION: u16 = 14;

/// Runs every upgrade a dump written at `version` needs over its decoded heap.
pub(crate) fn upgrade(version: u16, heap: &mut Heap, interns: &mut Interns) {
    if version <= dataclasses::LEGACY_DUMP_VERSION {
        dataclasses::upgrade_legacy_classes(heap, interns);
    }
    if version <= LEGACY_EVAL_OFFSETS_VERSION {
        rebase_legacy_eval_offsets(interns);
    }
}

/// Shifts every position in `eval()` code past the leading whitespace the old
/// build stripped, so it indexes the whole source as current positions do.
///
/// `exec()` shares the snippet source table but never stripped anything. An
/// eval source is recognised by failing to parse as a module, which indenting
/// the expression's line or a non-ASCII space before it causes. A source valid
/// as both (only blank or comment-only lines before unindented code) cannot be
/// told apart and is left as it was, so its positions can still point early.
fn rebase_legacy_eval_offsets(interns: &mut Interns) {
    let shifts: AHashMap<StringId, (u32, u32)> = interns
        .eval_sources()
        .filter_map(|(id, source)| {
            let shift = u32::try_from(source.len() - source.trim_start().len()).ok()?;
            let len = u32::try_from(source.len()).ok()?;
            (shift > 0 && parse_module(source).is_err()).then_some((id, (shift, len)))
        })
        .collect();
    if shifts.is_empty() {
        return;
    }
    interns.for_each_function_mut(|function| {
        let name = &mut function.name.position;
        // The snippet's own module function spans the whole source and was never relative.
        if let Some(&(shift, len)) = shifts.get(&name.filename)
            && !(name.start_byte == 0 && name.end_byte == len)
        {
            shift_range(name, shift);
        }
        function.code.for_each_position_mut(|range| {
            if let Some(&(shift, _)) = shifts.get(&range.filename) {
                shift_range(range, shift);
            }
        });
    });
}

/// Moves `range` `shift` bytes later in its source.
fn shift_range(range: &mut CodeRange, shift: u32) {
    range.start_byte = range.start_byte.saturating_add(shift);
    range.end_byte = range.end_byte.saturating_add(shift);
}
