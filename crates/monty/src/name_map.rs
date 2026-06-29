//! Ordered registry that maps namespace slots to interned names and vice versa.
//!
//! A `NameMap` is the single source of truth for a scope's name → slot
//! assignment. Slots are allocated sequentially as new names are seen
//! (`ensure_slot`), so the position in the registry IS the `NamespaceId`
//! that the compiler will encode into bytecode. The reverse direction
//! (`name_at`) gives the VM the `StringId` for a slot, which the runtime
//! uses to produce `NameError` / `UnboundLocalError` messages naming the
//! actual variable.
//!
//! Owning both directions in one structure is what fixes the historical
//! foot-gun where a scope kept a `name → slot` hashmap alongside a parallel
//! `namespace_size` counter: the two could drift, and there was no reverse
//! map at all — the VM used the wrong frame's `local_names` to label a
//! global-slot `NameError`.
//!
//! `NameMap` is keyed by [`StringId`] (the interner index), not `String`,
//! because every prepare-time site that allocates a slot already has the
//! `StringId` on hand from the AST. Avoiding `String::clone` per insert is
//! a measurable win for large modules.

use ahash::AHashMap;

use crate::{
    intern::StringId,
    namespace::NamespaceId,
    parse::{CodeRange, ParseError},
};

/// Ordered map from interned names to namespace slots.
///
/// Slots are dense and assigned in insertion order: the first inserted name
/// gets slot `0`, the second slot `1`, and so on. This means
/// `name_at(slot)` is a `Vec` index lookup and `len()` equals the next slot
/// that would be allocated — there is no separate `namespace_size`
/// counter to keep in sync.
///
/// Used for module globals, function locals, and any other scope that
/// needs a stable string → bytecode-slot mapping.
///
/// # Serialization
///
/// Only `slots` is serialized; `by_name` is reconstructed deterministically
/// on deserialization (see [`NameMapWire`]). Putting only the canonical
/// forward direction on the wire ensures untrusted snapshot input cannot
/// desync the two halves — every load goes through `try_from`, which also
/// enforces the `NamespaceId` (`u16`) upper bound on `slots.len()`.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(into = "NameMapWire", try_from = "NameMapWire")]
pub(crate) struct NameMap {
    /// Name interned at each slot, indexed by `NamespaceId::index()`.
    slots: Vec<StringId>,
    /// Reverse lookup so `ensure_slot` is O(1).
    by_name: AHashMap<StringId, NamespaceId>,
}

/// On-the-wire representation of a [`NameMap`].
///
/// Carries only the canonical `slots` direction so a deserialized `NameMap`
/// always has a `by_name` map that is a deterministic function of `slots`.
/// This eliminates an attacker-controlled inconsistency between the two
/// halves that the previous derive-based deserialization would have
/// accepted unconditionally.
#[derive(serde::Serialize, serde::Deserialize)]
struct NameMapWire {
    slots: Vec<StringId>,
}

impl From<NameMap> for NameMapWire {
    fn from(map: NameMap) -> Self {
        Self { slots: map.slots }
    }
}

impl TryFrom<NameMapWire> for NameMap {
    type Error = String;

    fn try_from(wire: NameMapWire) -> Result<Self, Self::Error> {
        // Refuse oversized slot vectors at the wire boundary: `NamespaceId`
        // is `u16`, so the bytecode slot operand cannot reach indices past
        // `u16::MAX + 1`. Without this check a malicious snapshot could
        // hand us an arbitrarily large vector and the subsequent
        // `NamespaceId::new` panic would surface as an internal error.
        let max_slots = usize::from(u16::MAX) + 1;
        if wire.slots.len() > max_slots {
            return Err(format!(
                "NameMap has too many slots: {} (maximum is {max_slots})",
                wire.slots.len(),
            ));
        }
        let mut by_name = AHashMap::with_capacity(wire.slots.len());
        for (idx, &name_id) in wire.slots.iter().enumerate() {
            // Safe by the length check above.
            let slot = NamespaceId::new(idx).expect("slot index fits in NamespaceId by length check");
            // First occurrence wins, matching the live `ensure_slot` /
            // `push_aliased_slot` invariant where a function parameter
            // shadows any later cell/free-var slot that reuses its
            // `StringId`.
            by_name.entry(name_id).or_insert(slot);
        }
        Ok(Self {
            slots: wire.slots,
            by_name,
        })
    }
}

impl NameMap {
    /// Returns an empty `NameMap` with no slots allocated.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns an empty `NameMap` with capacity reserved for `cap` slots.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            slots: Vec::with_capacity(cap),
            by_name: AHashMap::with_capacity(cap),
        }
    }

    /// Number of slots currently allocated.
    ///
    /// This is the size of the namespace and the slot id that `ensure_slot`
    /// would assign to the next inserted name.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Returns the slot already allocated for `name_id`, if any.
    pub fn get(&self, name_id: StringId) -> Option<NamespaceId> {
        self.by_name.get(&name_id).copied()
    }

    /// Returns `true` if a slot has been allocated for `name_id`.
    pub fn contains(&self, name_id: StringId) -> bool {
        self.by_name.contains_key(&name_id)
    }

    /// Returns the slot for `name_id`, allocating a new one if absent.
    ///
    /// `position` is used to anchor the `SyntaxError` raised when the scope
    /// would grow past the `u16` slot-index limit (`u16::MAX + 1` slots).
    /// Allocation is idempotent: calling `ensure_slot` twice with the same
    /// `name_id` returns the same slot.
    pub fn ensure_slot(&mut self, name_id: StringId, position: CodeRange) -> Result<NamespaceId, ParseError> {
        if let Some(&id) = self.by_name.get(&name_id) {
            return Ok(id);
        }
        let id = NamespaceId::new(self.slots.len()).ok_or_else(|| namespace_overflow(position))?;
        self.slots.push(name_id);
        self.by_name.insert(name_id, id);
        Ok(id)
    }

    /// Allocates a fresh slot whose reverse-map entry is `name_id`, WITHOUT
    /// touching the forward `name → slot` map.
    ///
    /// Used to back cell variables and free variables, which need a slot in
    /// the function's namespace distinct from any same-named parameter slot.
    /// For example, `def f(n): return lambda x: x + n` makes `n` both a
    /// parameter (slot 0) AND a cell variable (a fresh slot) — at call
    /// time the runtime copies `n`'s parameter value into the cell so the
    /// returned lambda's closure refers to the cell, not the now-gone stack
    /// slot.
    ///
    /// The slot's name is recorded in the reverse direction (slot →
    /// `name_id`) so the VM can label a `NameError` raised against the cell.
    /// The forward direction keeps the parameter as the canonical owner so
    /// `get(name_id)` still resolves to the param slot.
    pub fn push_aliased_slot(&mut self, name_id: StringId, position: CodeRange) -> Result<NamespaceId, ParseError> {
        let id = NamespaceId::new(self.slots.len()).ok_or_else(|| namespace_overflow(position))?;
        self.slots.push(name_id);
        Ok(id)
    }

    /// Iterates over `(slot, name)` pairs in slot order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (NamespaceId, StringId)> + '_ {
        self.slots.iter().enumerate().map(|(i, &name)| {
            // `i < slots.len() ≤ u16::MAX + 1` because every push went
            // through `ensure_slot`, which checks the `u16` overflow.
            let slot = NamespaceId::new(i).expect("slot index fits in NamespaceId by construction");
            (slot, name)
        })
    }
}

/// Builds the `ParseError` raised when a scope's namespace would exceed
/// `NamespaceId`'s `u16` capacity (the bytecode slot operand width).
/// Hoisted so the cold error path stays out of inlined allocator calls.
#[cold]
#[inline(never)]
pub(crate) fn namespace_overflow(position: CodeRange) -> ParseError {
    ParseError::syntax(
        format!(
            "too many distinct names in scope; maximum is {} per scope",
            (u16::MAX as usize) + 1
        ),
        position,
    )
}
