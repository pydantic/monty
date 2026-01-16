# Signature Binding Performance Plan

## Executive Summary

This document outlines a plan to optimize Monty's function argument binding to match CPython's performance characteristics. The key insight from CPython is that argument binding should be a simple memcpy-style operation in the common case, with minimal branching and zero heap allocations.

## CPython's Approach

Based on analysis of CPython's `initialize_locals()` in `ceval.c` and the vectorcall protocol (PEP 590):

### Key Design Decisions

1. **Vectorcall Protocol**: Arguments passed as `(PyObject *const *args, size_t nargsf, PyObject *kwnames)`
   - `args` is a pointer to a contiguous array of arguments
   - `nargsf` encodes the count (use `PyVectorcall_NARGS()` to extract)
   - `kwnames` is a tuple of keyword argument names (values follow positional args in the array)
   - **Zero allocation** for passing arguments - just pointer + counts

2. **Compact Code Object Metadata**: Just three integers describe the signature:
   - `co_argcount` - positional-or-keyword parameter count
   - `co_posonlyargcount` - positional-only parameter count
   - `co_kwonlyargcount` - keyword-only parameter count
   - Flags (`CO_VARARGS`, `CO_VARKEYWORDS`) indicate presence of `*args`/`**kwargs`

3. **Direct Copy for Positional Args**: Simple loop copying args directly to locals:
   ```c
   for (j = 0; j < n; j++) {
       localsplus[j] = args[j];
   }
   ```

4. **Keyword Matching via Pointer Comparison First**:
   ```c
   for (j = co->co_posonlyargcount; j < total_args; j++) {
       if (varname == keyword) goto kw_found;  // Fast: pointer comparison
   }
   // Fallback: string comparison
   ```

5. **Defaults Stored Separately**: `func_defaults` (tuple) and `func_kwdefaults` (dict) on function object, not interleaved with signature metadata.

## Current Monty Issues

From `signature-performance.md`:

1. **`args.split()` forces allocation**: Even `ArgValues::One` and `ArgValues::Two` get converted to `Vec<Value>` immediately in the complex path.

2. **Large `Signature` struct**: Uses `Option<Vec<StringId>>` for each parameter group, causing pointer chasing and poor cache locality.

3. **No fast path for common "complex" cases**: A function with just one default value falls into the full binding algorithm.

## Proposed Changes

### Phase 1: Eliminate Allocations in `bind()` (High Impact)

**Goal**: Ensure `ArgValues::One`, `ArgValues::Two`, and small argument lists never allocate.

#### 1.1 Create `ArgPosIter` - Zero-Allocation Positional Iterator

```rust
/// Iterator over positional arguments without allocation.
pub enum ArgPosIter {
    Empty,
    One(Option<Value>),
    Two(Option<Value>, Option<Value>),
    Vec(std::vec::IntoIter<Value>),
}

impl Iterator for ArgPosIter {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        match self {
            Self::Empty => None,
            Self::One(v) => v.take(),
            Self::Two(v1, v2) => v1.take().or_else(|| v2.take()),
            Self::Vec(iter) => iter.next(),
        }
    }
}
```

#### 1.2 Replace `split()` with `into_parts()`

```rust
impl ArgValues {
    pub fn into_parts(self) -> (ArgPosIter, KwargsValues) {
        match self {
            Self::Empty => (ArgPosIter::Empty, KwargsValues::Empty),
            Self::One(v) => (ArgPosIter::One(Some(v)), KwargsValues::Empty),
            Self::Two(v1, v2) => (ArgPosIter::Two(Some(v1), Some(v2)), KwargsValues::Empty),
            Self::ArgsKargs { args, kwargs } => (ArgPosIter::Vec(args.into_iter()), kwargs),
        }
    }
}
```

#### 1.3 Update `Signature::bind()` to use iterator

Replace:
```rust
let (positional_args, keyword_args) = args.split();
let mut pos_iter = positional_args.into_iter();
```

With:
```rust
let (mut pos_iter, keyword_args) = args.into_parts();
```

The rest of the binding logic using `.next()` remains unchanged.

### Phase 2: Expand Fast Paths (Medium Impact)

**Goal**: Handle more cases without the full binding algorithm.

#### 2.1 Add fast paths for common "complex" signatures

Many real functions have simple structures plus one extra feature:
- Positional args + one default
- Positional args + `*args`
- Positional args + one kwonly arg

Add specialized fast paths:

```rust
impl Signature {
    /// Fast path for: def f(a, b=default)
    /// Only positional-or-keyword args, some with defaults, no *args/**kwargs/kwonly
    fn is_simple_with_defaults(&self) -> bool {
        self.pos_args.is_none()
            && self.var_args.is_none()
            && self.kwargs.is_none()
            && self.var_kwargs.is_none()
    }

    /// Fast path for: def f(a, b, *args)
    /// Only positional-or-keyword args plus *args, no defaults/kwargs
    fn is_varargs_only(&self) -> bool {
        self.pos_args.is_none()
            && self.pos_defaults_count == 0
            && self.arg_defaults_count == 0
            && self.var_args.is_some()
            && self.kwargs.is_none()
            && self.var_kwargs.is_none()
    }
}
```

Then in `bind()`:
```rust
if self.is_simple() {
    return self.bind_simple(args, namespace, heap);
}
if self.is_simple_with_defaults() {
    return self.bind_simple_with_defaults(args, defaults, namespace, heap);
}
if self.is_varargs_only() {
    return self.bind_varargs_only(args, namespace, heap);
}
// Full algorithm...
```

### Phase 3: Compact Signature Representation (Medium Impact)

**Goal**: Improve cache locality and reduce memory usage for common signatures.

#### 3.1 Enum-based Signature with compact variants

```rust
/// Compact function signature representation.
///
/// Common cases use small inline variants; complex signatures use boxed storage.
pub enum Signature {
    /// No parameters: `def f(): ...`
    Empty,

    /// Single positional-or-keyword parameter: `def f(x): ...`
    One(StringId),

    /// Two positional-or-keyword parameters: `def f(x, y): ...`
    Two(StringId, StringId),

    /// Simple positional parameters (3+), no defaults or special params
    /// Stores count and offset into a names array in Code/Interns
    Simple { count: u8, names_offset: u16 },

    /// Full signature with all features
    Complex(Box<ComplexSignature>),
}

/// Full signature data for complex cases.
pub struct ComplexSignature {
    /// Positional-only parameter count (names stored in Code)
    pos_only_count: u8,
    /// Positional-or-keyword parameter count
    pos_or_kw_count: u8,
    /// Keyword-only parameter count
    kw_only_count: u8,
    /// Flags: has_varargs, has_varkwargs
    flags: u8,
    /// Number of positional-only params with defaults (from end)
    pos_defaults_count: u8,
    /// Number of pos-or-kw params with defaults (from end)
    arg_defaults_count: u8,
    /// Bitmap of which kwonly params have defaults (up to 64 kwonly args)
    kwarg_defaults_bitmap: u64,
    /// Parameter names stored inline (avoids indirection)
    names: Vec<StringId>,
}
```

**Benefits**:
- `Signature::Empty/One/Two` are 1-2 words, hot in cache
- Dispatch via match is faster than checking 6 boolean conditions
- `ComplexSignature` packs counts into single bytes (max 255 params is plenty)
- Bitmap for kwarg defaults avoids `Option<Vec<Option<usize>>>`

#### 3.2 Store parameter names by offset into shared storage

Instead of storing `Vec<StringId>` in every signature, store parameter names once in `Code` or `Interns`, and reference them by offset:

```rust
/// In Code object
pub struct Code {
    // ... existing fields ...

    /// All parameter names for functions in this module, concatenated.
    /// Individual signatures reference slices via (offset, count).
    param_names: Vec<StringId>,
}

/// In Signature::Simple
Simple { count: u8, names_offset: u16 }
// Access: code.param_names[names_offset..names_offset + count]
```

### Phase 4: Keyword Argument Optimization (Low Impact)

**Goal**: Speed up keyword argument matching.

#### 4.1 Use StringId comparison directly

Currently:
```rust
keyword_name.matches(*param_id, interns)
```

If both sides are interned (which they should be), this can be a direct integer comparison:

```rust
// In KeywordName enum
pub enum KeywordName {
    Interned(StringId),
    Heap(HeapId, /* cached_str */ String),
}

impl KeywordName {
    #[inline]
    fn matches_fast(&self, param_id: StringId) -> Option<bool> {
        match self {
            Self::Interned(id) => Some(*id == param_id),
            Self::Heap(..) => None, // Fall back to string comparison
        }
    }
}
```

#### 4.2 Consider perfect hashing for large signatures

For functions with many parameters (>8), build a minimal perfect hash at compile time mapping keyword names to parameter indices. This is likely not worth the complexity for typical Python code.

## Implementation Order

| Phase | Effort | Impact | Dependencies |
|-------|--------|--------|--------------|
| 1.1-1.3 (ArgPosIter) | Small | High | None |
| 2.1 (More fast paths) | Medium | Medium | None |
| 3.1 (Enum Signature) | Large | Medium | Careful refactoring |
| 3.2 (Shared name storage) | Medium | Low | Phase 3.1 |
| 4.1 (Fast StringId compare) | Small | Low | None |

**Recommended order**: 1.1-1.3 first (quick win), then 2.1, then evaluate if 3.x is worth the refactoring cost.

## Benchmarking Considerations

Benchmark these scenarios:
1. `f()` - No arguments
2. `f(x)` - One positional arg
3. `f(x, y)` - Two positional args
4. `f(x, y=1)` - One default
5. `f(x, *args)` - Varargs
6. `f(x, **kwargs)` - Kwargs
7. `f(a, b, c, d=1, e=2, *args, f, g=3, **kwargs)` - Full complexity

Current implementation allocates in cases 4-7 even when called with simple positional args. After Phase 1, cases 4-6 should be allocation-free when called simply.

## References

- [PEP 590 - Vectorcall](https://peps.python.org/pep-0590/)
- [CPython Call Protocol](https://docs.python.org/3/c-api/call.html)
- [CPython ceval.c](https://github.com/python/cpython/blob/main/Python/ceval.c) - `initialize_locals()` function
- [CPython code.h](https://github.com/python/cpython/blob/main/Include/cpython/code.h) - `PyCodeObject` struct
