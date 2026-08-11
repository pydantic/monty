---
include:
  - "crates/**/*.rs"
exclude:
  - "crates/monty-bench/**"
  - "crates/fuzz/**"
---

A `DropWithContext` value carries a cleanup obligation that Rust's own `Drop`
cannot discharge, because releasing it needs a context borrow (the heap, the VM)
that a bare `Drop` does not have. So the model is: once such a value is live, it
must reach a release on **every** exit -- normal return, `?`, `continue`,
`break`, panic -- and the way to guarantee that against all branches is to bind
it into a guard, not to hand-place a release in each branch.

Flag a `DropWithContext` value that stays live across a branch or early exit
with no guard covering it, and a guard used only by borrowing its contents
(`as_parts`/`as_parts_mut`) when it is never moved back out -- that case wants the
scope-bound form, not an explicit guard. Do not flag a single straight-line path
with no branch between acquiring and releasing, where a direct release is already
the clearest code. Rate a missed release (a real leak on some path) high; rate a
guard-style preference low.
