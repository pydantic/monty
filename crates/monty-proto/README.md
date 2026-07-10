# monty-proto

The wire protocol connecting [Monty](https://github.com/pydantic/monty) worker
processes to the parents that drive them.

Monty executes untrusted Python, and a Monty process can never be made fully
crash-proof against memory errors (stack overflow aborts, allocator aborts).
The subprocess architecture isolates those crashes: a parent — the
[`monty-pool`](https://crates.io/crates/monty-pool) crate, and through it the
Python and JavaScript packages — drives `monty subprocess` children over
framed stdio (or a WebSocket), and a dead child is simply replaced. This crate
defines the protocol both sides speak.

The protocol is protobuf (rather than Monty's internal postcard format) so a
parent or child can be implemented in any language — see
[`proto/monty/v1/monty.proto`](https://github.com/pydantic/monty/blob/main/crates/monty-proto/proto/monty/v1/monty.proto)
for the schema and the protocol rules documented alongside it. The
[`monty-cpython`](https://github.com/pydantic/monty/tree/main/crates/monty-cpython)
worker, which executes fed code in embedded CPython instead of Monty, speaks
this same protocol.

## What the crate provides

- `pb` — prost-generated message types. The generated code is checked in;
  regenerate with `make generate-proto` (CI enforces sync via
  `make check-proto`).
- `FrameReader` / `write_frame` — 4-byte little-endian length-prefixed
  framing, with a hard cap on frame length.
- Fallible conversions between `pb` types and Monty's public types
  (`MontyObject`, `MontyException`, mounts, resource limits, ...).
- `MONTY_VERSION` — the version both sides compare in the `Configure`
  handshake. The protocol has no in-band negotiation, so parent and child must
  be deployed in lockstep.
- `python` (cargo feature, off by default) — the `python` module: PyO3-based
  conversions between live Python objects and `MontyObject`/`MontyException`,
  shared by the `pydantic-monty` extension module and the `monty-cpython`
  embedded-CPython worker. The feature pulls in `pyo3` (but never its
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

This crate includes the `worker` feature and module

A transport-agnostic Monty protocol-child state machine, shared by the native subprocess and the wasm worker.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers. **this crate**
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
