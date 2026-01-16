# Review of sig-perf.md

## Summary

The document proposes optimizations to Monty's function argument binding based on CPython's vectorcall protocol. The analysis is mostly sound, but there are factual errors, omissions, and some proposed changes that are already implemented or unnecessary.

## Errors and Corrections

### 1. Reference to Non-Existent File

> From `signature-performance.md`:

This file doesn't exist in the repository. The document appears to reference analysis that was either deleted or never committed.

### 2. Phase 4.1 Is Already Implemented

The proposal in Phase 4.1 suggests:

> Currently: `keyword_name.matches(*param_id, interns)`
> If both sides are interned (which they should be), this can be a direct integer comparison

This is **already implemented**. See `src/value.rs:1320-1325`:

```rust
impl EitherStr {
    pub fn matches(&self, target: StringId, interns: &Interns) -> bool {
        match self {
            Self::Interned(id) => *id == target,  // Direct integer comparison
            Self::Heap(s) => s == interns.get_str(target),
        }
    }
}
```

The `EitherStr::Interned` case already does `*id == target`, which is the exact optimization proposed. Phase 4.1 can be removed from the plan.

### 3. Misleading Code Comments

In Phase 2.1, the proposed `is_simple_with_defaults()` has comment:

> `/// Fast path for: def f(a, b=default)`

But the implementation checks `self.pos_args.is_none()`. This is confusing because `pos_args` holds *positional-only* parameters (parameters before `/`), not regular positional parameters. A function like `def f(a, b=1)` stores its parameters in `self.args`, not `self.pos_args`. The code is technically correct but the comment is misleading.

## Omissions

### 1. `bound_params` Vec Allocation

The document doesn't mention that `Signature::bind()` allocates a `Vec<bool>` on every call:

```rust
let mut bound_params = vec![false; all_named_slots];
```

For functions with many parameters, this is a per-call allocation that could be:
- Eliminated for simple cases (already tracked by position in `pos_iter`)
- Replaced with a `u64` bitmap for functions with <= 64 parameters (covers 99%+ of real functions)

### 2. `KwargsValuesIter::Dict` Allocates

In `src/args.rs:227`, the `Dict` variant allocates:

```rust
Self::Dict(dict) => KwargsValuesIter::Dict(dict.into_iter().collect::<Vec<_>>().into_iter()),
```

This converts the dict iterator to a `Vec` and back to an iterator. It should iterate directly over the dict without the intermediate allocation.

### 3. Bytecode VM Integration

The document doesn't consider how signature binding integrates with the bytecode VM migration. The `Code` struct already stores `local_names: Vec<StringId>` - there's an opportunity to:
- Store parameter names in `Code` once and reference by slice
- Avoid duplicating signature metadata between `Signature` and `Code`
- Potentially inline simple binding in the VM's `CALL` instruction

### 4. `cleanup_on_error` Inefficiency

Every error path in `bind()` calls `cleanup_on_error()`, which iterates over the namespace and individually drops values. For simple cases, the namespace only contains values that were passed in - they don't need explicit cleanup if the caller handles them. This matters for error-heavy paths.

## Potential Improvements

### 1. Inline More Fast Paths Into `is_simple()`

The current `is_simple()` is very restrictive. Consider expanding to cover:

```rust
/// Fast path: no *args/**kwargs/kwonly, may have defaults but called with exact arg count
fn can_use_simple_bind(&self, positional_count: usize, has_kwargs: bool) -> bool {
    !has_kwargs
        && self.pos_args.is_none()
        && self.var_args.is_none()
        && self.kwargs.is_none()
        && self.var_kwargs.is_none()
        && positional_count == self.arg_count()  // Exact match, no need for defaults
}
```

This would allow `def f(a, b=1)` called as `f(1, 2)` to use the fast path.

### 2. Use SmallVec or Inline Storage for `bound_params`

Most functions have < 8 parameters. Use a type that avoids allocation for small cases:

```rust
// Option A: Bitmap for <= 64 params
let mut bound_params: u64 = 0;
// Set: bound_params |= 1 << i;
// Check: (bound_params & (1 << i)) != 0

// Option B: SmallVec
let mut bound_params: SmallVec<[bool; 8]> = smallvec![false; all_named_slots];
```

### 3. Simplify Phase 3 (Enum Signature)

The proposed enum-based `Signature` with `Empty`, `One`, `Two`, `Simple`, `Complex` variants adds significant complexity. Consider a simpler approach:

```rust
pub struct Signature {
    /// Packed counts: [pos_only_count, pos_or_kw_count, kw_only_count, flags]
    counts: [u8; 4],
    /// All parameter names in slot order (single allocation)
    names: Vec<StringId>,
    /// Default info (separate, loaded lazily)
    defaults_info: DefaultsInfo,
}
```

This avoids the multiple `Option<Vec<>>` indirections without the complexity of an enum with many variants.

### 4. Avoid Re-checking `is_simple()` Conditions

The `bind()` method checks `is_simple()`, then if it falls through to the full path, re-derives many of the same values (e.g., counts). Consider returning structured data from the simple-path check:

```rust
enum BindStrategy {
    Simple,
    SimpleWithDefaults { first_optional: usize },
    Full,
}
```

## Implementation Priority Recommendation

| Change | Effort | Impact | Notes |
|--------|--------|--------|-------|
| Phase 1 (ArgPosIter) | Small | High | Correct, implement as proposed |
| Fix KwargsValuesIter::Dict | Small | Medium | Simple fix, removes allocation |
| Remove Phase 4.1 | None | N/A | Already implemented |
| `bound_params` bitmap | Small | Medium | Easy win for common cases |
| Expand simple fast path | Medium | High | Cover more common signatures |
| Phase 3 (Signature enum) | Large | Low | Questionable ROI, defer |

## Code Quality Notes

1. The document references CPython's `initialize_locals()` - this is good context but the citation should include the specific version (the code structure changes between Python versions).

2. The proposed `ArgPosIter::Two` implementation has a subtle correctness issue:
   ```rust
   Self::Two(v1, v2) => v1.take().or_else(|| v2.take()),
   ```
   This is correct but worth adding a comment that `take()` on v1 is guaranteed to return `Some` on first call, so the order is deterministic.

3. Phase 3.2 (storing parameter names by offset into Code) is a good idea but should be deferred until after the bytecode VM migration is complete, as the storage model may change.

## Conclusion

The Phase 1 changes (ArgPosIter and `into_parts()`) are well-designed and should be implemented as proposed - they eliminate allocations in the common path with minimal code changes.

Phase 2 (expanded fast paths) is worthwhile but the specific implementations need refinement to match the actual struct field semantics.

Phase 3 adds significant complexity for questionable benefit and should be deferred or simplified.

Phase 4.1 is already implemented and should be removed from the plan. Phase 4.2 (perfect hashing) is correctly identified as over-engineering and should remain deprioritized.

The document should be updated to:
1. Remove reference to non-existent `signature-performance.md`
2. Remove Phase 4.1 (already done)
3. Add `bound_params` optimization to Phase 1
4. Add `KwargsValuesIter::Dict` fix to Phase 1
5. Clarify integration points with bytecode VM migration
