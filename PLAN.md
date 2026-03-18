# HeapReader Branch Cleanup Plan

## Context

The `HeapRead`/`HeapReader` pattern has been introduced. `Value` methods (`py_eq`, `py_add`,
`py_getitem`, `py_setitem`, `py_call_attr`, etc.) now dispatch through `HeapRead` instead of
the old `Heap::get()` -> `HeapData` -> `PyTrait` chain. This leaves a significant amount of
dead code in the old dispatch path.

## Goal

Remove dead `PyTrait` methods and their per-type implementations that are no longer reachable,
and slim `PyTrait` to only the methods still used through `HeapData`.

## Dead `HeapData` Dispatch Methods

The following `PyTrait` method implementations on `HeapData` (in `impl_py_trait_dispatch!` in
`heap_data.rs`) are never called -- `Value` dispatches through `HeapRead` instead:

- `py_eq` (~60 lines)
- `py_add` (~18 lines)
- `py_sub` (~20 lines)
- `py_mod` (~19 lines)
- `py_mod_eq` (~11 lines)
- `py_iadd` (~10 lines)
- `py_getitem` (~13 lines)
- `py_setitem` (~13 lines)

## Per-Type `PyTrait` Methods to Remove

Once the `HeapData` dispatch methods above are removed, the per-type `PyTrait` implementations
they dispatched to become dead too. Remove these for every type that now has `HeapRead`
equivalents:

### List (`list.rs`)

- `List::py_getitem` (~28 lines)
- `List::py_setitem` (~53 lines)
- `List::py_eq` (~14 lines)
- `List::py_add` (~11 lines)
- `List::py_iadd` (~45 lines)
- `List::py_call_attr` (~20 lines)
- `call_list_method` fn + all its helper fns: `list_insert`, `list_pop`, `list_remove`,
  `list_clear`, `list_copy`, `list_extend`, `list_index`, `list_count` (~300 lines total)
- `List::append(&mut self, heap, item)` -- the direct version (only called from the dead
  `call_list_method` and `list_extend` paths)
- `List::insert(&mut self, heap, index, item)` -- same situation

### Dict (`dict.rs`)

- `Dict::py_getitem`
- `Dict::py_setitem`
- `Dict::py_eq`
- `Dict::py_add`
- `Dict::py_iadd`
- `Dict::py_call_attr` + its helper fns (old dispatch path)

### Tuple (`tuple.rs`)

- `Tuple::py_getitem`
- `Tuple::py_eq`
- `Tuple::py_add`
- `Tuple::py_cmp`
- `Tuple::py_call_attr`

### Set (`set.rs`)

- `Set::py_eq`
- `Set::py_sub`
- `Set::py_call_attr`
- `FrozenSet::py_eq`
- `FrozenSet::py_sub`
- `FrozenSet::py_call_attr`

### Other Types

- `DictKeysView::py_call_attr`, `DictItemsView::py_call_attr`, `DictValuesView::py_call_attr`
- `Dataclass::py_call_attr`
- `Path::py_call_attr`
- `Str::py_call_attr`
- `Bytes::py_call_attr`
- `RePattern::py_call_attr`
- `ReMatch::py_call_attr`

### Verify Before Removing

For each type, confirm no remaining callers exist outside the dead `HeapData` dispatch path.
The types that still need `py_getitem`/`py_setitem` on the raw type (e.g. `Range::py_getitem`,
`NamedTuple::py_getitem`) should be checked -- if they're only called from the dead `HeapData`
dispatch and have `HeapRead` equivalents, they can go too.

## Slim `PyTrait` Itself

After removing the dead methods from all implementers, remove the method declarations from the
`PyTrait` trait. The trait should be left with only the methods still used through `HeapData`:

**Keep:**
- `py_type`
- `py_len`
- `py_bool`
- `py_repr_fmt` / `py_repr` / `py_str`
- `py_dec_ref_ids`
- `py_estimate_size`

**Remove from trait:**
- `py_eq`
- `py_add`
- `py_sub`
- `py_mod`
- `py_mod_eq`
- `py_iadd`
- `py_mult` (default impl only)
- `py_div` (default impl only)
- `py_floordiv` (default impl only)
- `py_pow` (default impl only)
- `py_getitem`
- `py_setitem`
- `py_call_attr`
- `py_cmp`

This prevents dead dispatch from silently coming back.

## Estimated Savings

~400-600 lines removed from the diff.

## Approach

1. Remove the 8 dead methods from `impl_py_trait_dispatch!` in `heap_data.rs`
2. Compiler errors will identify which per-type `PyTrait` impls are now missing -- remove them
3. Compiler warnings (`dead_code`) will identify helper fns only reachable from the removed
   methods -- remove those too
4. Remove the method declarations from the `PyTrait` trait itself
5. Run `make format-rs && make lint-rs` to clean up
6. Run `make test-ref-count-panic` to verify nothing broke
