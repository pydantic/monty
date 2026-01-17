# Signature Binding Performance Optimization Plan

## Executive Summary

This plan consolidates insights from all four sig-perf reports and codebase exploration to create a practical optimization strategy for Monty's function argument binding. The key wins come from eliminating allocations in the common path.

## Key Findings From Analysis

### Confirmed Issues (High Impact)
1. **`args.split()` allocates unnecessarily** - `args.rs:76-84` converts `ArgValues::One/Two` to `Vec<Value>` even when iteration would suffice
2. **`bound_params` Vec allocation** - `signature.rs:328` allocates `vec![false; all_named_slots]` on every complex call
3. **`KwargsValuesIter::Dict` double allocation** - `args.rs:227` collects dict iterator to Vec, then back to iterator
4. **`EitherStr::Heap` allocation** - `value.rs:1293` calls `.to_owned()` for every heap string during keyword matching

### Already Implemented
- **Phase 4.1 (StringId comparison)** - `EitherStr::matches()` at `value.rs:1319-1325` already does direct integer comparison for interned strings

### Deprioritized
- **Perfect hashing for keywords** - Over-engineering for typical Python code

---

## Implementation Plan

### Phase 1: Eliminate Allocations in bind() (HIGH IMPACT)

#### 1.1 Create `ArgPosIter` - Zero-Allocation Positional Iterator

**File**: `crates/monty/src/args.rs`

```rust
/// Iterator over positional arguments without allocation.
///
/// Supports iterating over `ArgValues::One/Two` without converting to Vec.
/// Must be fully consumed or explicitly dropped with `drop_remaining_with_heap()`
/// to maintain correct reference counts.
pub enum ArgPosIter {
    Empty,
    One(Option<Value>),
    Two(Option<Value>, Option<Value>),
    Vec(std::vec::IntoIter<Value>),
}

impl ArgPosIter {
    /// Drop any remaining values in the iterator.
    /// MUST be called if iteration is abandoned early (e.g., on error).
    pub fn drop_remaining_with_heap(self, heap: &mut Heap<impl ResourceTracker>) {
        for value in self {
            value.drop_with_heap(heap);
        }
    }
}

impl Iterator for ArgPosIter {
    type Item = Value;

    #[inline]
    fn next(&mut self) -> Option<Value> {
        match self {
            Self::Empty => None,
            Self::One(v) => v.take(),
            Self::Two(v1, v2) => v1.take().or_else(|| v2.take()),
            Self::Vec(iter) => iter.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Empty => (0, Some(0)),
            Self::One(v) => {
                let n = v.is_some() as usize;
                (n, Some(n))
            }
            Self::Two(v1, v2) => {
                let n = v1.is_some() as usize + v2.is_some() as usize;
                (n, Some(n))
            }
            Self::Vec(iter) => iter.size_hint(),
        }
    }
}

impl ExactSizeIterator for ArgPosIter {}
```

#### 1.2 Add `into_parts()` method to `ArgValues`

**File**: `crates/monty/src/args.rs`

```rust
impl ArgValues {
    /// Split into positional iterator and keyword values without allocating
    /// for One/Two cases.
    pub fn into_parts(self) -> (ArgPosIter, KwargsValues) {
        match self {
            Self::Empty => (ArgPosIter::Empty, KwargsValues::Empty),
            Self::One(v) => (ArgPosIter::One(Some(v)), KwargsValues::Empty),
            Self::Two(v1, v2) => (ArgPosIter::Two(Some(v1), Some(v2)), KwargsValues::Empty),
            Self::Kwargs(kwargs) => (ArgPosIter::Empty, kwargs),
            Self::ArgsKargs { args, kwargs } => (ArgPosIter::Vec(args.into_iter()), kwargs),
        }
    }
}
```

#### 1.3 Update `Signature::bind()` to use `into_parts()`

**File**: `crates/monty/src/signature.rs:296`

Replace:
```rust
let (positional_args, keyword_args) = args.split();
let mut pos_iter = positional_args.into_iter();
```

With:
```rust
let (mut pos_iter, keyword_args) = args.into_parts();
```

**CRITICAL**: All error paths must call `pos_iter.drop_remaining_with_heap(heap)` before returning to avoid reference count leaks.

#### 1.4 Replace `bound_params` Vec with u64 bitmap

**File**: `crates/monty/src/signature.rs:328`

Replace:
```rust
let mut bound_params = vec![false; all_named_slots];
```

With:
```rust
let mut bound_params: u64 = 0;
// Set: bound_params |= 1 << i
// Check: (bound_params & (1 << i)) != 0
```

**Limit enforcement**: Functions with >64 parameters should return a `SyntaxError` at parse/compile time. Add validation in `Signature::from_params()` to check total parameter count and return a syntax error if exceeded. This matches Python's practical limits and eliminates the need for fallback code.

This eliminates the per-call Vec allocation for all functions.

#### 1.5 Fix `KwargsValuesIter::Dict` allocation

**File**: `crates/monty/src/args.rs:227`

Replace:
```rust
Self::Dict(dict) => KwargsValuesIter::Dict(dict.into_iter().collect::<Vec<_>>().into_iter()),
```

With direct dict iteration (requires changing `KwargsValuesIter::Dict` to hold the dict iterator directly):
```rust
Self::Dict(dict) => KwargsValuesIter::Dict(dict.into_iter()),
```

This requires updating `KwargsValuesIter::Dict` variant to:
```rust
Dict(crate::heap::DictIter),  // or whatever the dict's IntoIterator type is
```

---

### Phase 2: Expand Fast Paths (MEDIUM IMPACT)

#### 2.1 Add `can_use_simple_bind()` method

**File**: `crates/monty/src/signature.rs`

The current `is_simple()` is too restrictive. Add a method that checks if simple binding can be used based on *both* the signature AND the call arguments:

```rust
impl Signature {
    /// Check if we can use simple binding for this call.
    ///
    /// Simple binding works when:
    /// - No *args/**kwargs/kwonly parameters
    /// - Positional arg count matches exactly (no defaults needed)
    /// - No keyword arguments passed
    fn can_use_simple_bind(&self, pos_count: usize, has_kwargs: bool) -> bool {
        !has_kwargs
            && self.pos_args.is_none()      // No positional-only params
            && self.var_args.is_none()      // No *args
            && self.kwargs.is_none()        // No keyword-only params
            && self.var_kwargs.is_none()    // No **kwargs
            && pos_count == self.arg_count() // Exact match, defaults not needed
    }
}
```

This allows `def f(a, b=1)` called as `f(1, 2)` to use the fast path.

#### 2.2 Add fast path for functions with only defaults

**File**: `crates/monty/src/signature.rs`

Add specialized handling for the common case: positional-or-keyword params with some defaults, no special params:

```rust
/// Fast path for: def f(a, b=1, c=2) - only pos-or-kw params with defaults
fn is_simple_with_defaults(&self) -> bool {
    self.pos_args.is_none()
        && self.var_args.is_none()
        && self.kwargs.is_none()
        && self.var_kwargs.is_none()
        // Has some defaults, otherwise is_simple() would match
        && self.arg_defaults_count > 0
}
```

Implement `bind_simple_with_defaults()` that:
1. Fills positional args from iterator
2. Fills remaining from defaults
3. No `bound_params` tracking needed

---

### Phase 3: Compact Signature Enum (MEDIUM IMPACT)

The current `Signature` struct uses multiple `Option<Vec<StringId>>` fields causing:
- Multiple pointer indirections
- Poor cache locality (parameter names scattered across heap)
- Large struct size (~120 bytes)

Replace with an enum-based design that inlines common cases:

**File**: `crates/monty/src/signature.rs`

```rust
/// Compact function signature representation.
///
/// Common cases use small inline variants; complex signatures use boxed storage.
/// All variants enforce the 64-parameter limit at construction time.
pub enum Signature {
    /// No parameters: `def f(): ...`
    Empty,

    /// Single positional-or-keyword parameter: `def f(x): ...`
    One(StringId),

    /// Two positional-or-keyword parameters: `def f(x, y): ...`
    Two(StringId, StringId),

    /// Three positional-or-keyword parameters: `def f(x, y, z): ...`
    Three(StringId, StringId, StringId),

    /// Full signature: 4+ params, defaults, pos-only, *args, **kwargs, or kwonly
    Complex(Box<ComplexSignature>),
}

/// Full signature data for all non-trivial cases.
pub struct ComplexSignature {
    /// All parameter names in namespace order: [pos_only][pos_or_kw][kwonly]
    names: Vec<StringId>,
    /// Positional-only count
    pos_only_count: u8,
    /// Positional-or-keyword count
    pos_or_kw_count: u8,
    /// Keyword-only count
    kw_only_count: u8,
    /// Flags: bit 0 = has_varargs, bit 1 = has_varkwargs
    flags: u8,
    /// *args name (if present)
    var_args: Option<StringId>,
    /// **kwargs name (if present)
    var_kwargs: Option<StringId>,
    /// Defaults: [pos_defaults_count, arg_defaults_count, kwarg_defaults_bitmap_low, kwarg_defaults_bitmap_high]
    defaults_info: [u8; 4],
    /// Bitmap for which kwonly params have defaults (supports up to 64 kwonly args)
    kwarg_defaults_bitmap: u64,
}
```

**Benefits**:
- `Signature::Empty/One/Two/Three` are 1-3 words, hot in cache
- Most real functions (0-3 simple params) use inline variants
- `ComplexSignature` consolidates all names into single Vec
- Dispatch via match replaces multiple Option checks
- Total struct size: 8-24 bytes for inline variants vs ~120 bytes before

**Implementation approach**:
1. Add new types alongside existing `Signature`
2. Create `Signature::from_params()` that builds appropriate variant
3. Implement `bind()` methods for each variant
4. Migrate call sites one by one
5. Remove old `Signature` struct

---

### Phase 4: Documentation and Cleanup (LOW EFFORT)

1. Remove Phase 4.1 from `sig-perf.md` - already implemented
2. Remove reference to non-existent `signature-performance.md`
3. Add comments explaining the fast path conditions
4. Document reference count safety requirements in `ArgPosIter`

---

## Files to Modify

| File | Changes |
|------|---------|
| `crates/monty/src/args.rs` | Add `ArgPosIter`, replace `split()` with `into_parts()`, fix `KwargsValuesIter::Dict` |
| `crates/monty/src/signature.rs` | Restructure to enum, implement specialized `bind()` per variant, add 64-param limit, use bitmap |
| `crates/monty/src/function.rs` | Update function creation to use new Signature API |
| `crates/monty/src/lib.rs` | Re-export new Signature types if needed |
| `sig-perf.md` | Update to reflect completed/removed items |

---

## Verification Plan

1. **Run existing tests**: `make test-ref-count-panic` - all tests must pass
2. **Verify reference counting**: Check that `refcount__*.py` tests still pass
3. **Run function tests**: All `function__*.py` test cases must pass
4. **Manual benchmarking**: Create benchmark comparing:
   - `f()` - no args
   - `f(x)` - one arg
   - `f(x, y)` - two args
   - `f(x, y=1)` called with `f(1)` vs `f(1, 2)` - defaults
   - `f(x, *args)` - varargs
   - `f(**kwargs)` - keyword args

---

## Risk Mitigation

### Reference Count Leaks
- **Risk**: `ArgPosIter` not fully consumed on error paths
- **Mitigation**: Add `drop_remaining_with_heap()` method, audit all error paths

### Fast Path Correctness
- **Risk**: Fast path conditions don't exactly match full algorithm semantics
- **Mitigation**: Extensive existing test suite, add edge case tests

### 64 Parameter Limit
- **Risk**: Existing code may have >64 parameters
- **Mitigation**: Return `SyntaxError` at compile time with clear message. This matches Python's practical limits and is a reasonable restriction for a sandboxed interpreter.

---

## Deferred Work

1. **Phase 4.2 (Perfect hashing)** - Over-engineering for typical Python code.
2. **Shared name storage in Code** - Better to tackle during bytecode VM migration.
