# Plan: `&self` Allocation for Heap

## Goal

Make `Heap::allocate` (and related methods like `get_empty_tuple`) work with `&self` instead of `&mut self`. This unlocks allocating new heap entries while holding shared borrows to existing heap data — currently impossible because `allocate` requires `&mut Heap`, which conflicts with any outstanding `&self` borrows.

This plan works directly on `main`. It does **not** depend on the `dh/heap-reader-2` branch or its `HeapReader`/`HeapRead` API. The take-and-restore pattern (`take_data!`/`restore_data!`, `with_entry_mut`, `with_two`) remains unchanged — eliminating those is a separate future effort that can build on the stable-address foundation established here.

## Why This Is Worth Doing

Many VM operations follow the pattern "read operands → compute → allocate result." Today, `allocate(&mut self)` conflicts with any shared borrow of heap data, forcing workarounds. With `allocate(&self)`:

1. **New code can hold `&self` borrows across allocations** — e.g., reading two strings to concatenate then allocating the result, without releasing the reads first.
2. **Prerequisite for `HeapReader`/`HeapRead`** — the `dh/heap-reader-2` branch's pointer-based borrowing API requires stable addresses (from `HeapEntries`) and `&self` allocation to be useful. This plan delivers both without the complexity of the full `HeapRead` migration.
3. **Future: separate Allocator handle** — could eventually split `Heap` into a read-only view + allocator handle, enabling more granular borrow splitting.

## Step-by-Step Plan

### Step 1: Introduce `HeapEntries`

Replace `entries: Vec<Option<HeapValue>>` and `free_list: Vec<HeapId>` with a new `HeapEntries` struct in a dedicated `heap_entries.rs` file (beside `heap.rs`). This keeps all paged-storage unsafe code in its own module with a minimal public API, forming a clear safety boundary. `HeapEntries` owns both the paged storage and the free list.

```rust
const PAGE_SIZE: usize = 256;

struct HeapEntries {
    pages: Vec<Box<[MaybeUninit<Option<HeapValue>>]>>,
    len: usize,
    free_list: Vec<HeapId>,
}
```

Key properties:
- Each page is a `Box<[MaybeUninit<...>]>` of `PAGE_SIZE` entries, heap-allocated once and **never moved**.
- Growing the storage appends a new `Box` to the `pages` vec — existing page contents are unaffected.
- Only initialized slots (indices `< len`) are accessed; new pages are allocated without zeroing.
- Index `i` maps to `pages[i / PAGE_SIZE][i % PAGE_SIZE]`.
- The free list tracks freed slot IDs for reuse — populated by `dec_ref`, consumed by `allocate`.

Grouping the free list with the paged storage is natural: both are implementation details of slot management. This keeps `Heap` focused on higher-level concerns (resource tracking, GC scheduling, refcounting) while `HeapEntries` encapsulates the storage layer. In step 4, both `pages` and `free_list` need `UnsafeCell` wrapping — co-locating them in `HeapEntries` keeps all the unsafe interior mutability in one struct with one set of documented invariants.

The API:

```rust
impl HeapEntries {
    fn new() -> Self;
    fn with_capacity(capacity: usize) -> Self;
    fn len(&self) -> usize;
    fn get(&self, index: usize) -> Option<&HeapValue>;
    fn get_mut(&mut self, index: usize) -> &mut Option<HeapValue>;
    fn push(&mut self, value: Option<HeapValue>) -> usize;
    fn try_get_mut(&mut self, index: usize) -> Option<&mut Option<HeapValue>>;
    fn iter(&self) -> impl Iterator<Item = &Option<HeapValue>>;
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut Option<HeapValue>>;

    /// Allocates a slot (reusing from free list or appending) and returns its ID.
    /// Takes `&mut self` initially (step 1), relaxed to `&self` in step 4.
    fn allocate(&mut self, value: HeapValue) -> HeapId;

    /// Returns a freed slot to the free list. Always `&mut self`.
    fn free(&mut self, id: HeapId);
}
```

All existing `self.entries[i]` access becomes `self.entries.get(i)` / `self.entries.get_mut(i)`, and `self.free_list.pop()`/`self.free_list.push()` moves into `HeapEntries::allocate`/`HeapEntries::free`. The `Heap` serde impls serialize entries as a flat `Vec<Option<HeapValue>>` + `Vec<HeapId>` (unchanged wire format) and reconstruct `HeapEntries` on deserialization.

This step is a pure internal refactor — no API changes for callers of `Heap`.

**This is the key structural change that makes `&self` allocation sound.** Once pages never move, we can safely write to new/freed slots via `&self` without invalidating any existing reference into a page.

**Reference implementation:** Commit `2901d96` ("lazily initialize pages") on the `dh/heap-reader-2` branch introduced `PagedEntries` with the same paged `MaybeUninit` design. That implementation can be used as a starting point — it covers `push`, `get`, `get_mut`, `iter`, `iter_mut`, `Drop`, and `with_capacity`. The main addition here is co-locating the `free_list` and renaming to `HeapEntries`.

**Risk:** Low. Straightforward translation from `Vec` indexing to paged indexing. `MaybeUninit` requires a few `unsafe` blocks for `assume_init_ref`/`assume_init_mut`, but the invariant is simple: all slots at indices `< len` are initialized.

### Step 2: Change `ResourceTracker::on_allocate` and `on_free` to `&self`

The `on_allocate` and `on_free` methods currently take `&mut self`. Change both to `&self`:

```rust
pub trait ResourceTracker: fmt::Debug {
    fn on_allocate(&self, get_size: impl FnOnce() -> usize) -> Result<(), ResourceError>;
    fn on_free(&self, get_size: impl FnOnce() -> usize);
    // ... rest unchanged
}
```

Implementation changes:
- `NoLimitTracker` — both are no-ops already, just change the signature.
- `LimitedTracker` — wrap `allocation_count: usize` and `current_memory: usize` in `Cell<usize>`. The `check_counter` field already uses `AtomicU16` for interior mutability, so this follows the existing pattern.

Serde: `Cell<usize>` doesn't derive `Serialize`/`Deserialize`. Add manual impls or use `#[serde(with = "...")]` helpers. Since `LimitedTracker` already derives both, the simplest approach is to switch to a manual impl that calls `.get()` for serialization and `Cell::new(v)` for deserialization.

**Risk:** Low. Single-threaded only, so `Cell` is sufficient. The `allocation_count()` and `current_memory()` accessor methods just change from `self.field` to `self.field.get()`.

### Step 3: Add interior mutability to `Heap` scalar fields

Change these fields from plain values to `Cell`:

| Field | Before | After |
|---|---|---|
| `may_have_cycles` | `bool` | `Cell<bool>` |
| `allocations_since_gc` | `u32` | `Cell<u32>` |

`recursion_depth` already uses `Cell<usize>` — no change needed.

Update the `Heap` serde impls to call `.get()` / `Cell::new(v)` for these fields. The existing custom `Serialize`/`Deserialize` impls already extract values manually, so this is straightforward.

**Risk:** None. Purely mechanical.

### Step 4: Add interior mutability to `HeapEntries`

This is the core unsafe change. Wrap the mutable collections in `UnsafeCell`/`Cell`:

```rust
struct HeapEntries {
    pages: UnsafeCell<Vec<Box<[MaybeUninit<Option<HeapValue>>]>>>,
    len: Cell<usize>,
    free_list: UnsafeCell<Vec<HeapId>>,
}
```

Relax `HeapEntries::allocate` from `&mut self` to `&self`. The body is unchanged — it pops from the free list or appends — but now operates through the `UnsafeCell`/`Cell` wrappers:

```rust
impl HeapEntries {
    /// Allocates a slot, reusing from free list or appending.
    ///
    /// # Safety contract (enforced by caller structure, not runtime checks):
    /// - No `&mut` reference to `pages` or `free_list` exists (guaranteed because
    ///   this is only called from `Heap::allocate` which holds `&self`, not `&mut self`).
    /// - New slots (at index `len`) have never been initialized — no existing
    ///   reference can point to them.
    /// - Reused slots (from free list) were freed and have no active borrows.
    fn allocate(&self, value: HeapValue) -> HeapId;
}
```

Existing `&mut self` methods (`get_mut`, `iter_mut`, `try_get_mut`, `free`) remain unchanged — they are used by `dec_ref`, GC, and other operations that genuinely need exclusive access.

**Safety argument for `pages: UnsafeCell<Vec<...>>`:**
- **Slot writes** go to `MaybeUninit` memory that has never been initialized (new slot) or to a freed slot with no active borrows. No existing reference is affected.
- **Vec growth** (`pages.push(new_page)`) reallocates the `Vec<Box<...>>` buffer — but this only moves the *page pointers* (the `Box` values), not the page contents. Any existing `&HeapValue` reference points into a `Box`'s heap allocation, not into the `Vec`'s buffer. Safe as long as we don't hold references into the `Vec` itself during growth.
- **`get(&self)`** reads through the `UnsafeCell` with a shared reference. Safe because the only concurrent mutation is appending to the vec or writing to *different* slots — the slot at index `len` is never readable (it's past the end), and freed slots being written have no active borrows.

**Key invariant:** `allocate(&self)` only writes to the slot at index `len` (new) or a freed slot from the free list (no active borrows). It never touches slots that have active `&HeapValue` references.

**Safety argument for `free_list: UnsafeCell<Vec<HeapId>>`:**
- Only accessed during `allocate` (pop, via `&self`) and `free` (push, via `&mut self`).
- These never run concurrently: `allocate` needs `&self` and `free` needs `&mut self` — the borrow checker prevents overlap.
- No other code holds references to the free list contents.

**Risk:** This is the riskiest step. Mitigations: document invariants on every `unsafe` block, keep the unsafe surface minimal (one `allocate_shared` method that touches both `UnsafeCell` fields).

### Step 5: Change `Heap::allocate` and `get_empty_tuple` to `&self`

With all fields wrapped in interior mutability, change the signatures:

```rust
pub fn allocate(&self, data: HeapData) -> Result<HeapId, ResourceError>;
pub fn get_empty_tuple(&self) -> Value;
```

`inc_ref` already takes `&self` — no change needed.

`allocate` body becomes:
```rust
self.tracker.on_allocate(|| data.py_estimate_size())?;
if data.is_gc_tracked() {
    self.allocations_since_gc.set(self.allocations_since_gc.get().wrapping_add(1));
    if data.has_refs() {
        self.may_have_cycles.set(true);
    }
}

let new_entry = HeapValue { ... };
let id = self.entries.allocate(new_entry);
Ok(id)
```

### Step 6: Keep `dec_ref` and `collect_garbage` as `&mut self`

These operations genuinely need exclusive access:
- `dec_ref` frees slots, pushes to free list, and recursively processes child references.
- `collect_garbage` iterates all entries and frees unreachable ones.

Both remain `&mut self`. This is correct — the borrow checker ensures you can't allocate (which now only needs `&self`) while `dec_ref` is running (which needs `&mut self`), preventing races on the free list.

## Serde Implications

`Cell<usize>`, `Cell<u32>`, and `Cell<bool>` don't implement `Serialize`/`Deserialize` by default. The existing custom serde impls for `Heap` already extract values manually, so adapting them is straightforward — call `.get()` during serialization and wrap in `Cell::new()` during deserialization.

`UnsafeCell<Vec<...>>` similarly needs manual handling in serde — access via `.get()` for serialization, wrap in `UnsafeCell::new()` for deserialization.

The wire format does not change.

## What This Does NOT Change

- **`take_data!`/`restore_data!` macros** — remain as-is. Eliminating these requires the `HeapRead` pointer-based API from `dh/heap-reader-2`, which is a separate effort.
- **`with_entry_mut`/`with_two`** — remain as-is, same reason (16+ `with_entry_mut` call sites, 3 `with_two` call sites).
- **`HeapValue::data: Option<HeapData>`** — remains optional. Removing the `Option` wrapper requires `HeapRead`'s approach of using `UnsafeCell<HeapData>` with pointer-based access.
- **`HeapReader`/`HeapRead`** — not introduced in this plan. They can be added later, building on the stable-address `HeapEntries` foundation.

## What This Unlocks

1. **`&self` allocation while holding shared borrows** — code that has `&Heap` (or `&self` on a type containing `Heap`) can now allocate without needing `&mut Heap`.
2. **Foundation for `HeapReader`/`HeapRead`** — `HeapEntries` provides the address stability that `HeapRead`'s `NonNull` pointers require. The `&self` allocation means future `HeapRead` users can allocate while holding reads.
3. **Incremental adoption** — existing code continues to work unchanged. New code can start using `&self` allocation patterns immediately.

## Simplification Opportunities

Once `allocate(&self)` lands, the following can be simplified in follow-up PRs.

### Opportunity 1: `get_empty_tuple` is needlessly `&mut self`

`heap.rs` — `get_empty_tuple(&mut self)` only calls `inc_ref()`, which already takes `&self`. The `&mut` is purely historical. Changing it to `&self` is trivial once `allocate` no longer forces `&mut` on the surrounding API.

### Opportunity 2: 50+ functions take `&mut VM` / `&mut Heap` solely to call `allocate()`

These functions read input data, compute a result, then allocate. The only heap mutation is `allocate()`. With `allocate(&self)`, these can take `&VM` / `&Heap`:

**String methods** (`types/str.rs`) — `str_lower`, `str_upper`, `str_replace`, `str_capitalize`, `str_title`, `str_swapcase`, `str_zfill`, `str_center`, `str_ljust`, `str_rjust`, `str_join`, `str_split`, `str_encode`, etc. (~30 functions, now taking `&mut VM` after PR #277)

**Bytes methods** (`types/bytes.rs`) — `bytes_lower`, `bytes_upper`, `bytes_capitalize`, `bytes_title`, `bytes_swapcase`, `bytes_zfill`, `bytes_fromhex`, etc. (~20 functions, now taking `&mut VM` after PR #277)

**Collection operations:**
- `types/list.rs` — `list_copy`, `getitem_slice`
- `types/range.rs` — `getitem_slice`
- `types/tuple.rs` — `allocate_tuple`
- `types/dict.rs` — `dict_popitem`
- `types/long_int.rs` — `into_value`

After PR #277, many of these take `&mut VM` instead of `&mut Heap` directly. The same principle applies: they could become `&VM` since the only mutation is allocation through `vm.heap`.

### Opportunity 3: Eliminate clone-to-release-borrow patterns

Four locations clone data just to release `&Heap` before calling `allocate`:

- `types/str.rs` — clones a `Slice` before `getitem_slice`
- `types/list.rs` — same pattern
- `types/range.rs` — same pattern
- `types/type.rs` — clones string to `String` before `parse_int_from_str`

With `allocate(&self)`, the `&Heap` borrow from reading the slice can coexist with the allocation call — no clone needed.

### Opportunity 4: Eliminate collect-then-allocate patterns in VM collections

`bytecode/vm/collections.rs` has 5+ places where items are collected into a temporary `Vec`/`SmallVec` just to release the heap borrow before allocating:

- `list_extend` — clones list items into `Vec`, then extends
- `list_to_tuple` — clones list items into `SmallVec`, then `allocate_tuple`
- `dict_merge` — copies dict entries, then inserts
- `dict_update` — same pattern
- `set_extend` — copies set items, then extends

With `allocate(&self)`, these could iterate directly from the source while allocating into the target, avoiding the intermediate collection.

### Opportunity 5: `mark_potential_cycle` becomes `&self` for free

Since `may_have_cycles` becomes `Cell<bool>`, `mark_potential_cycle(&mut self)` can take `&self`. This cascades to callers like `List::append` and `List::insert` in `types/list.rs`.

### Not fixed by this plan (needs `HeapRead` later)

The `with_entry_mut` / `with_two` / `take_data!` / `restore_data!` patterns exist because a closure needs `&mut VM` while also accessing entry data — the conflict is between reading entry data and holding `&mut Heap` for *any* mutation (not just allocation). Eliminating these requires the `HeapRead` pointer-based API, which is a separate follow-up that builds on the `HeapEntries` stable-address foundation established here.

## Migration Strategy

Steps 1-5 should be one PR. Step 1 (introduce `HeapEntries`) is the largest change but is a pure refactor with no API impact. Steps 2-5 are smaller and build on each other.

After merging, the simplification opportunities above can be pursued in follow-up PRs — each is independent and low-risk.

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `UnsafeCell` in `HeapEntries` allows aliasing bugs | Document invariants on every `unsafe` block; `allocate(&self)` only touches the `len` slot (no reader can access it) or freed slots (no active borrows); all unsafe is localized in `HeapEntries` |
| `Vec::push` on `pages` during `allocate_shared(&self)` while `get(&self)` holds a reference into a page | Safe: `Vec` growth reallocates the pointer array, not the page contents. `get` holds a reference into a `Box`'s heap allocation, not into the `Vec`'s buffer |
| Serde breakage from `Cell`/`UnsafeCell` wrappers | Custom impls already exist, just need updating. Wire format unchanged. |
| `Send`/`Sync` implications of `UnsafeCell` | `Heap` already contains `Cell` (which is `!Sync`). Single-threaded use only. No change in capabilities. |
| `MaybeUninit` in `HeapEntries` introduces unsoundness | Simple invariant: all slots at indices `< len` are initialized via `push`. Bounded number of `unsafe` blocks (get, get_mut, push, iter, drop). |
