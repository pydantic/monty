---
include:
  - "crates/**/*.rs"
exclude:
  - "crates/monty-bench/**"
  - "crates/fuzz/**"
---

Monty runs untrusted, potentially malicious Python, so the model to apply is:
input that crosses a trust boundary is hostile until validated, and the cost of
a defect scales with which side of the boundary it lands on.

Two boundaries carry hostile input: sandboxed Python reaching Rust, and a wire
frame arriving from a child process (`monty-proto` proto->Rust conversion).
Anything reachable from either must validate its input and cannot
`unwrap`/`expect`/panic, index unchecked, or overflow an integer feeding a
length or index; and it must not let sandboxed code reach the host filesystem
outside a mount, the network, or a subprocess. A snapshot or dump the host loads
through its own trusted, transport-verified path is trusted by contract, so
absent validation there is not a finding -- but a dump or snapshot arriving from
a subprocess child is hostile (a child can mint its own) and must be validated or
rejected as a protocol violation like any other child frame.

Calibrate severity by blast radius, not by the bug in isolation. The same panic
is contained in a pool worker -- the child dies, the parent replaces it and
raises -- so rank it lower; but in host or parent code (`monty-pool`,
`monty-proto` decoding, `monty-fs`, the `monty-python`/`monty-js` bindings) or in
the `monty` crate embedded in-process, it takes down the caller, so rank it
high, and treat a confirmed sandbox escape or a memory-safety defect (use-after-
free, aliasing violation, out-of-bounds) as critical. `heap.rs` and
`path_security.rs` are the load-bearing safety files; hold changes to them to
that critical bar.
