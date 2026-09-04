# monty-proto

The wire protocol connecting [Monty](https://github.com/pydantic/monty) worker
processes to the parents that drive them.

Monty executes untrusted Python, and a Monty process can never be made fully
crash-proof against memory errors (stack overflow aborts, allocator aborts —
the [`monty-alloc`](https://crates.io/crates/monty-alloc) allocator turns the
latter into this crate's `OOM_EXIT_CODE` so a parent can classify them).
The subprocess architecture isolates those crashes: a parent — the
[`monty-pool`](https://crates.io/crates/monty-pool) crate, and through it the
Python and JavaScript packages — drives `monty subprocess` children over
framed stdio (or a WebSocket), and a dead child is simply replaced. This crate
defines the protocol both sides speak.

The protocol is protobuf (rather than Monty's internal postcard format) so a
parent or child can be implemented in any language — see
[`proto/monty/v1/monty.proto`](https://github.com/pydantic/monty/blob/main/crates/monty-proto/proto/monty/v1/monty.proto)
for the schema and the protocol rules documented alongside it.

## What the crate provides

- `pb` — prost-generated message types. The generated code is checked in;
    regenerate with `make generate-proto` (CI enforces sync via
    `make check-proto`).
- `FrameReader` / `write_frame` — 4-byte little-endian length-prefixed
    framing, with a hard cap on frame length.
- Fallible conversions between `pb` types and Monty's public types
    (`MontyObject`, `MontyException`, mounts, resource limits, ...).
- Host-object routing on the wire: host-backed `MontyObject::ClassInstance` /
    `MontyClassType` carry host-generated uuids, and `FunctionCall.object_id` /
    `NameLookup.object_id` route their method calls and lazy attribute lookups
    back to the parent's per-session instance store; sandbox-defined classes and
    instances carry worker-generated uuids that never reach that store.
- `PROTOCOL_VERSION` / `MIN_SUPPORTED_PROTOCOL_VERSION` — the wire schema
    version a parent declares in `Configure`, and the range a child serves.
    Versioned independently of the monty package: peers on different releases
    interoperate as long as their protocol versions overlap. There is no in-band
    negotiation, so a child rejecting a version reports its range in the
    `FatalError` for the parent to downgrade to.
- `python` (cargo feature, off by default) — the `python` module: PyO3-based
    conversions between live Python objects and `MontyObject`/`MontyException`,
    used by the `pydantic-monty-client` extension module. The feature pulls in `pyo3` (but never its
    `extension-module` feature — how libpython is linked stays the top crate's
    decision), so pure-Rust consumers pay nothing for it.

## Values are special-cased for performance

The `monty.v1.MontyObject` message is mapped via prost `extern_path` onto
`WireObject`: a hand-written `prost::Message` implementation that encodes
borrowed `MontyObject`s and validates *while* decoding — no mirror struct and
no deep clone on the hot path. `tests/differential.rs` proves it
byte-compatible against a fully prost-generated oracle (`tests/oracle/`,
regenerated and CI-checked together with the main codegen).

## Children are untrusted

A parent must treat every frame from a (possibly compromised) child as
untrusted input: conversions from proto to Rust are fallible by design,
decoding enforces depth and size budgets, and nothing in this crate panics on
malformed wire data.

## Worker state machine

The `worker` cargo feature (off by default) adds the `worker` module: the
transport-agnostic child state machine, shared by the native `monty subprocess`
worker and the wasm worker. It links the `monty` interpreter, so only
worker-side crates enable it.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers. **this crate**
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
