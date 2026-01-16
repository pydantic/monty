# sig-perf review (codex)

## Findings (ordered by severity)
- High: `ArgPosIter` avoids allocation but does not cover refcount-safe cleanup on early error paths; leftover `Value`s must be dropped with `drop_with_heap`, so a guard or `drop_remaining_with_heap` is required to avoid leaks. `sig-perf.md:58`
- Medium: The proposed fast-path predicates do not explicitly exclude positional-only or keyword-only parameters or verify that defaults are present, which risks routing a signature into the wrong binder and producing incorrect error semantics. `sig-perf.md:125`
- Medium: Packing counts into `u8` and stating "max 255 params is plenty" is risky; Python allows larger signatures in some cases, so this should be guarded with an explicit limit check or widened to `u16` to avoid silent truncation. `sig-perf.md:191`
- Low: The claim of "zero allocation for passing arguments" is slightly overstated; vectorcall avoids allocations in the callee, but callers still allocate `kwnames`/argument arrays in common Python-level calls, so wording should be tightened. `sig-perf.md:13`
- Low: Phase 3.1 keeps `names: Vec<StringId>` in `ComplexSignature` while Phase 3.2 introduces shared name storage; this split likely leaves duplication in the complex path and should be reconciled. `sig-perf.md:191`

## Advice / improvements
- Add an explicit "drop remaining args" hook for positional and keyword iterators so refcounts are corrected even when binding fails mid-way.
- Tighten fast-path gating to check `pos_only_count == 0`, `kw_only_count == 0`, and the relevant defaults counts before dispatching to specialized binders.
- If you keep `u8` packing, introduce a compile-time or runtime guard with a clear error message; otherwise, prefer `u16` for counts to match Python's practical limits.
- Consider a single shared name storage strategy (offset + count) for both simple and complex signatures to minimize duplication and improve cache locality.
- If keyword argument handling still allocates in common cases, evaluate a small inline storage for `KwargsValues` similar to the positional optimization.

## Questions / assumptions
- Are there existing binding error paths that already drop remaining args via a guard or scope helper, and can `ArgPosIter` integrate with that without changing lifetimes?
- Do current call sites ever produce `ArgValues::ArgsKargs` with a pre-allocated `Vec` even for small arities, and if so can that be reduced upstream?
