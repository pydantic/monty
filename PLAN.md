# Plan: Replace `with_entry_mut` / `with_two` with `HeapRead`

## Background

`Heap::with_entry_mut` and `Heap::with_two` use a "take-and-restore" pattern: they temporarily remove `HeapData` from the heap entry, pass it to a closure along with `&mut VM`, then restore it. This avoids Rust borrow conflicts but has re-entrancy problems — if the closure triggers another `with_entry_mut` on the *same* ID, the data is `None` and the VM panics.

The new `HeapReader` / `HeapRead` API avoids this by using pointer-based access with lifetime-invariant guards. A `HeapRead<'h, T>` is a lightweight handle that borrows data through the `HeapReader`, accessed via `get(&self, heap)` / `get_mut(&mut self, heap)`. Multiple `HeapRead` handles can coexist (replacing `with_two`), and the data is never removed from the heap.

## `HeapRead` method pattern

Methods are implemented directly on `HeapRead<'h, T>` via `impl<'h> HeapRead<'h, T>` blocks, giving natural method call syntax. The `Dict` implementation demonstrates this:

```rust
impl<'h> HeapRead<'h, Dict> {
    pub fn set(
        &mut self,
        key: Value,
        value: Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<Option<Value>> {
        self.get_mut(vm.heap).contains_refs = true;
        let (opt_index, hash) = self.find_index_hash(&key, vm)?;
        // ...
    }

    fn find_index_hash(
        &self,
        key: &Value,
        vm: &mut VM<'h, '_, impl ResourceTracker>,
    ) -> RunResult<(Option<usize>, u64)> {
        // collect candidates via hash, then py_eq each
    }
}
```

Call site usage:
```rust
let HeapReadOutput::Dict(mut dict) = this.heap.read(dict_id) else {
    unreachable!("...");
};
if let Some(old_value) = dict.set(key, value, this)? {
    old_value.drop_with_heap(this);
}
```

Key points:
- Methods on `HeapRead<'h, T>` use `self.get(vm.heap)` / `self.get_mut(vm.heap)` to access the underlying `T`
- The `'h` lifetime ties the `HeapRead` to the `HeapReader` and `VM`, preventing escapes
- `&mut self` methods can interleave reads (`self.get(vm.heap)`) with VM mutations (allocation, `py_eq`, etc.)
- Private helpers (like `find_index_hash`) live in the same `impl` block
- Each type keeps its own `impl<'h> HeapRead<'h, T>` block in its own module (e.g., `dict.rs`)

## Inventory of all call sites

### `Heap::with_entry_mut` (22 call sites)

#### `collections.rs` — collection-building opcodes

| # | Location | Usage | Complexity |
|---|----------|-------|------------|
| 1 | `collections.rs:326` | `dict_update` — `dict.set(key, value)` in loop | Medium — same as dict_merge |
| 2 | `collections.rs:406` | `set_extend` — `set.add(item)` in loop | Medium — needs `HeapRead<Set>::add` |
| 3 | `collections.rs:441` | `list_append` — `list.append(value)` | Easy — simple append, no re-entrancy |
| 4 | `collections.rs:469` | `set_add` — `set.add(value)` | Medium — needs `HeapRead<Set>::add` |
| 5 | `collections.rs:500` | `dict_set_item` — `dict.set(key, value)` | Medium — same as dict_merge |

#### `binary.rs` — binary/set operators

| # | Location | Usage | Complexity |
|---|----------|-------|------------|
| 6 | `binary.rs:409` | `set_binary_op` — `set.binary_op_value(rhs, op)` | Hard — set binary ops read `rhs` which may also be on heap |

#### `value.rs` — core value operations

| # | Location | Usage | Complexity |
|---|----------|-------|------------|
| 7 | `value.rs:707` | `py_iadd` — in-place add for `Ref(id)` | Hard — `py_iadd` on HeapData dispatches to list/bytes/etc |
| 8 | `value.rs:1286` | `py_getitem` — `data.py_getitem(key)` | Hard — dispatches to list/dict/tuple getitem, may call `py_eq` |
| 9 | `value.rs:1346` | `py_setitem` — `data.py_setitem(key, value)` | Hard — dispatches to list/dict setitem |
| 10 | `value.rs:1519` | `py_contains` — outer container dispatch | Hard — dispatches to list/tuple/dict/set/str contains with `py_eq` |
| 11 | `value.rs:1537` | `py_contains` — nested `DictKeysView` → dict lookup | Hard — nested `with_entry_mut` |
| 12 | `value.rs:1551` | `py_contains` — nested `DictItemsView` → dict lookup | Hard — nested `with_entry_mut` |
| 13 | `value.rs:1562` | `py_contains` — nested `DictValuesView` → dict values | Hard — nested `with_entry_mut` |
| 14 | `value.rs:1633` | `py_getattr` — `data.py_getattr(attr)` | Hard — dispatches to many types, may allocate |
| 15 | `value.rs:1677` | `py_set_attr` — dataclass `dc.set_attr(name, value)` | Medium — only dataclass, limited dispatch |

#### `dict_view.rs` — dict view comparisons

| # | Location | Usage | Complexity |
|---|----------|-------|------------|
| 16 | `dict_view.rs:80` | `DictKeysView::eq_set` — dict keys vs set | Medium — iterates dict keys, calls `set.contains` |
| 17 | `dict_view.rs:99` | `DictKeysView::eq_frozenset` — dict keys vs frozenset | Medium — same pattern as eq_set |
| 18 | `dict_view.rs:118` | `DictKeysView::to_set` — materialize keys to set | Easy — just clones keys |
| 19 | `dict_view.rs:259` | `DictItemsView::eq_set` — dict items vs set | Medium |
| 20 | `dict_view.rs:278` | `DictItemsView::eq_frozenset` — dict items vs frozenset | Medium |
| 21 | `dict_view.rs:296` | `DictItemsView::to_set` — materialize items to set | Easy — clones + allocates tuples |

#### `list.rs` — tests only

| # | Location | Usage | Complexity |
|---|----------|-------|------------|
| 22-24 | `list.rs:871,901,929` | Tests — `py_setitem` on list | N/A — tests, update after API change |

### `Heap::with_two` (7 call sites)

| # | Location | Usage | Complexity |
|---|----------|-------|------------|
| 1 | `value.rs:239` | `py_eq` — `Ref(id1)` vs `Ref(id2)` | Medium — reads both, calls `py_eq` on HeapData |
| 2 | `value.rs:294` | `py_cmp` — tuple comparison | Easy — only for tuples |
| 3 | `value.rs:451` | `py_add` — `Ref + Ref` | Medium — dispatches to type-specific add |
| 4 | `value.rs:545` | `py_sub` — `Ref - Ref` | Medium — LongInt subtraction |
| 5 | `value.rs:600` | `py_mod` — `Ref % Ref` | Medium — LongInt modulo |
| 6 | `dict_view.rs:70` | `DictKeysView::eq_view` — keys view vs keys view | Medium — iterates + `py_eq` |
| 7 | `dict_view.rs:234` | `DictItemsView::eq_view` — items view vs items view | Medium — iterates + `py_eq` |

## Migration strategy

### Phase 1: Implement `HeapRead` methods for container mutations (Easy/Medium)

The existing `impl<'h> HeapRead<'h, Dict>` with `set` and `find_index_hash` is the template. Add equivalent `impl<'h> HeapRead<'h, T>` blocks for other container types.

**Step 1a:** Add `impl<'h> HeapRead<'h, Set>` with `add` and `contains` methods (needed for set_extend, set_add)

**Step 1b:** Add `impl<'h> HeapRead<'h, List>` with `append` method (needed for list_append — trivial since append doesn't call `py_eq`)

**Step 1c:** Convert `collections.rs` call sites #1–5:
- `dict_update` (site 1) → `heap.read(dict_id)` then `dict.set(key, value, vm)`
- `set_extend` (site 2) → `heap.read(set_id)` then `set.add(item, vm)`
- `list_append` (site 3) → `heap.read(list_id)` then `list.append(value, vm)` or direct push
- `set_add` (site 4) → `heap.read(set_id)` then `set.add(value, vm)`
- `dict_set_item` (site 5) → `heap.read(dict_id)` then `dict.set(key, value, vm)`

**Step 1d:** Convert `dict_view.rs` sites #16–21:
- `to_set` (sites 18, 21) → `heap.read(dict_id)` then iterate via `dict.get(vm.heap)`
- `eq_set` / `eq_frozenset` (sites 16, 17, 19, 20) → `heap.read(dict_id)` then iterate with contains check

**Step 1e:** Convert `value.rs:1677` — `py_set_attr` for dataclass (site 15)
- Add `impl<'h> HeapRead<'h, Dataclass>` with `set_attr` method

### Phase 2: Replace `with_two` with dual `HeapRead` (Medium)

Two `HeapRead` handles can coexist — `heap.read(id1)` and `heap.read(id2)` return independent handles. Access data via `handle.get(vm.heap)`.

**Step 2a:** Convert arithmetic operations (sites 1–5 of `with_two`):
- `py_eq` at `value.rs:239`
- `py_cmp` at `value.rs:294`
- `py_add` at `value.rs:451`
- `py_sub` at `value.rs:545`
- `py_mod` at `value.rs:600`

Pattern:
```rust
// Before:
Heap::with_two(vm, *id1, *id2, |vm, left, right| left.py_add(right, vm))

// After:
let left = vm.heap.read(*id1);
let right = vm.heap.read(*id2);
left.get(vm.heap).py_add(right.get(vm.heap), vm)
```

Note: `PyTrait` methods like `py_add`/`py_eq` currently take `&mut VM`. For types that need VM access during comparison (e.g., tuples calling element-wise `py_eq`), add `impl<'h> HeapRead<'h, Tuple>` with methods that can interleave `get()` calls with VM operations.

**Step 2b:** Convert dict view comparisons (sites 6–7 of `with_two`):
- `DictKeysView::eq_view` at `dict_view.rs:70`
- `DictItemsView::eq_view` at `dict_view.rs:234`

### Phase 3: Convert complex dispatch sites (Hard)

These sites use `with_entry_mut` to dispatch through `HeapDataMut` trait methods that need `&mut VM`. Replace with `heap.read()` + match on `HeapReadOutput` variant + type-specific `HeapRead` methods.

**Step 3a:** `py_getattr` (site 14) — `data.py_getattr(attr, vm)`
- Each type's `py_getattr` may allocate on the heap
- Solution: `heap.read(id)`, match on `HeapReadOutput`, call `HeapRead<T>::py_getattr` per type

**Step 3b:** `py_getitem` (site 8) — `data.py_getitem(key, vm)`
- Dict getitem calls `py_eq` for key lookup → add `HeapRead<Dict>::get` method
- List/tuple getitem with slice keys read the slice from heap
- Solution: add `HeapRead<T>::py_getitem` for each container type

**Step 3c:** `py_setitem` (site 9) — `data.py_setitem(key, value, vm)`
- Dict setitem already has `HeapRead<Dict>::set`
- List setitem needs slice support → add `HeapRead<List>::py_setitem`

**Step 3d:** `py_contains` (site 10) — big match on container type
- Add `HeapRead<Dict>::get`, `HeapRead<Set>::contains` for key/element lookup
- List/tuple: iterate via `handle.get(vm.heap).as_slice()`, call `py_eq` per element
- Nested dict view sites (11–13): use double `heap.read()` — one for the view, one for the dict

**Step 3e:** `py_iadd` (site 7) — in-place operations
- List iadd extends from another iterable → `HeapRead<List>::iadd`
- Bytes iadd concatenates → `HeapRead<Bytes>::iadd` or simpler approach

**Step 3f:** `set_binary_op` (site 6 of `with_entry_mut`) — set operations reading rhs
- `HeapRead<Set>::binary_op_value` and `HeapRead<FrozenSet>::binary_op_value`

### Phase 4: Update tests and remove old API

**Step 4a:** Update `list.rs` test sites (22–24) to use new API

**Step 4b:** Remove `Heap::with_entry_mut` and `Heap::with_two`

**Step 4c:** Remove `HeapDataMut` if no longer needed (it was the "taken out" mutable wrapper)

**Step 4d:** Remove `take_data!` / `restore_data!` macros

## Key design decisions

1. **Methods on `HeapRead<'h, T>`**: Methods live directly on `HeapRead<'h, T>` via `impl<'h> HeapRead<'h, T>` blocks in each type's module. This gives natural `handle.method(args, vm)` syntax and keeps type-specific logic co-located with the type. Each type module (e.g., `dict.rs`, `set.rs`, `list.rs`) owns its own `HeapRead` impl block.

2. **`HeapReadOutput` for dispatch**: The `HeapReadOutput` enum replaces `HeapDataMut` for match-based dispatch. Call sites do `let HeapReadOutput::Dict(mut dict) = heap.read(id) else { ... }` when the type is known, or match on all variants for generic dispatch (like `py_getattr`).

3. **Two reads at once**: For `with_two` replacements, two `HeapRead` handles coexist. The borrow checker ensures safety through the `HeapReader` lifetime.

4. **Incremental migration**: Each phase can be landed independently. The old `with_entry_mut` / `with_two` coexist with the new `HeapRead` pattern during migration.

## Ordering recommendation

Phase 1 → Phase 2 → Phase 3 → Phase 4

Start with the easy collection sites to build out the `HeapRead<T>` method library, then tackle the two-entry reads, then the complex dispatch sites. Remove the old API only after everything is converted.
