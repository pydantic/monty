# Review of Signature Binding Performance Plan

## Summary of Findings

The proposed plan in `sig-perf.md` is technically sound and accurately identifies the key bottlenecks in Monty's current argument binding implementation. The analysis of CPython's approach is relevant, and the translation to Rust idioms is largely correct.

**Key Validations:**
*   **Phase 1 (Eliminate Allocations)**: The diagnosis that `args.split()` causes unnecessary allocations is correct. The `ArgPosIter` solution is an excellent, idiomatic Rust approach to provide a unified iterator interface without heap allocation for the common `One`/`Two` cases.
*   **Phase 3 (Compact Signature)**: The current `Signature` struct is indeed large and cache-inefficient due to multiple `Option<Vec<StringId>>` fields. Flattening this is a significant optimization.

## Detailed Advice & Refinements

### 1. Implementation Details for `ArgPosIter`
The proposed implementation for `ArgPosIter::Two` using `v1.take().or_else(|| v2.take())` is correct and clever, but ensure it is robust.
*   **Suggestion**: Verify `ArgPosIter` behaves correctly if `next()` is called repeatedly after exhaustion (it should consistently return `None`). The current logic supports this.

### 2. Dependency Injection for Phase 3.2 (Shared Name Storage)
The plan proposes storing parameter names in `Code` or `Interns` and referencing them by offset in `Signature`.
*   **Issue**: `Signature::bind` currently relies on the `Signature` struct owning the parameter names (via `StringId`) to generate error messages (e.g., "missing argument 'x'").
*   **Refinement**: If names are removed from `Signature`, `bind()` will need access to the name storage. You will likely need to change the `bind` signature to accept a `param_names: &[StringId]` slice or a reference to the `Code` object, in addition to `Interns`. This is a non-trivial architectural change that affects all call sites.

### 3. Handling `KwargsValues`
The plan mentions `KwargsValues` in Phase 1.2 but doesn't detail it.
*   **Observation**: `KwargsValues` is currently an enum (`Empty`, `Inline`, `Dict`). The `into_parts` implementation needs to return this enum directly.
*   **Optimization**: Ensure `KwargsValues::Inline` (which uses a `Vec`) is not eagerly converted to a `Dict` or cloned during `bind` unless necessary.

### 4. Benchmarking Strategy
The plan lists *what* to benchmark but not *how*.
*   **Tooling**: Use `criterion` (already in the project) to create micro-benchmarks.
*   **Metrics**: Focus on **instructions** and **L1/L2 cache misses** (if available via `criterion-perf` or similar), not just wall time. The reduction in allocation should be visible in memory stats.
*   **Baseline**: Establish a baseline with the current `args.split()` implementation before applying Phase 1.

### 5. Testing
*   **Regression Tests**: The binding logic is complex. Ensure existing tests in `crates/monty/src/test_cases/` cover all edge cases (varargs, kwonly, defaults, mixed).
*   **New Tests**: Add specific unit tests for `ArgPosIter` to ensure it yields exactly the correct sequence and handles `take()` semantics properly.

### 6. Phase 4.2 (Perfect Hashing)
*   **Recommendation**: De-prioritize. For Python, the overhead of computing the hash for a small number of arguments often exceeds the cost of a linear scan or a simple interned string comparison, especially since arguments are usually passed by position.

## Proposed Action Plan Adjustment

1.  **Phase 1 (Immediate)**: Implement `ArgPosIter` and `into_parts`. Update `bind` to use the iterator. This is a purely internal change to `bind` and `ArgValues` with no external API breakage.
2.  **Benchmark**: Verify the win.
3.  **Phase 2 (Fast Paths)**: Implement `is_simple_with_defaults` inside `bind`.
4.  **Phase 3 (Refactoring)**: Tackle the `Signature` struct compaction. Be prepared to refactor `bind`'s call signature to support external name storage.

## Code Snippet Refinement (ArgPosIter)

```rust
impl Iterator for ArgPosIter {
    type Item = Value;

    #[inline]
    fn next(&mut self) -> Option<Value> {
        match self {
            Self::Empty => None,
            Self::One(v) => v.take(),
            // Logic: if v1 is Some, take it.
            // If v1 is None (already taken), try v2.
            Self::Two(v1, v2) => v1.take().or_else(|| v2.take()),
            Self::Vec(iter) => iter.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Empty => (0, Some(0)),
            Self::One(v) => {
                let len = if v.is_some() { 1 } else { 0 };
                (len, Some(len))
            },
            Self::Two(v1, v2) => {
                let len = (if v1.is_some() { 1 } else { 0 }) + (if v2.is_some() { 1 } else { 0 });
                (len, Some(len))
            },
            Self::Vec(iter) => iter.size_hint(),
        }
    }
}
```
Adding `size_hint` allows consumers (like `Vec::from_iter` or `collect`) to pre-allocate correctly if needed, though we aim to avoid collection.
