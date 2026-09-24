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

The protocol is protobuf (rather than Monty's internal CBOR dump format) so a
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
  (`MontyObject`/`CallArgs`/`NamedValues` over a `MontyGraph`,
  `MontyException`, mounts, resource limits, ...).
- Host-object routing on the wire: host-backed `ClassInstance` / `ClassType`
  nodes carry host-generated uuids, and `FunctionCall.object_id` /
  `NameLookup.object_id` route their method calls and lazy attribute lookups
  back to the parent's per-session instance store; sandbox-defined classes and
  instances carry worker-generated uuids that never reach that store.
- `PROTOCOL_VERSION` / `MIN_SUPPORTED_PROTOCOL_VERSION` — the wire schema
  version a parent declares in `Configure`, and the range a child serves.
  Versioned independently of the monty package: peers on different releases
  interoperate as long as their protocol versions overlap. There is no in-band
  negotiation, so a child rejecting a version reports its range in the
  `FatalError` for the parent to downgrade to, then exits non-zero.
  Version `0` is always rejected; `monty_version` is diagnostic metadata, not a compatibility check.
  Snapshot compatibility is checked separately using the dump-format version.
- `python` (cargo feature, off by default) — the `python` module: PyO3-based
  conversions between live Python objects and `MontyObject`/`MontyException`,
  used by the `pydantic-monty-client` extension module. The feature pulls in `pyo3` (but never its
  `extension-module` feature — how libpython is linked stays the top crate's
  decision), so pure-Rust consumers pay nothing for it.

Repeated fields and byte buffers in protocol messages use `BudgetVec<T>`.
Construct them from standard vectors with `.into()` or collect an iterator directly; use `.into_inner()` to recover a standard vector without copying.
Host construction and cloning are unbudgeted; fallible `try_push` charges any growth to the active decode budget.

## Values are special-cased for performance

Values cross as one flat `monty.v1.Arena` per message: a post-order node arena in which containers hold child indexes.
A sub-object shared inside the sandbox, or between two arguments of one call, is sent once, and the carrying message
names its roots by index.
prost `extern_path` maps the message onto `WireArena`, a hand-written `prost::Message` implementation that encodes
borrowed `monty_types::unstable::MontyNode`s without cloning and decodes one generated protobuf node at a time.
Each node is validated and converted before being retained in the domain arena; temporary payloads and conversion allocations share the frame budget.
Strings, bytes and reference buffers transfer without copying; reference containers use `WireIndexes`, `WireNodePairs` and `WireNamedTuple` rather than temporary vectors of protobuf ids.
Duplicate fields follow protobuf merging rules, and value depth does not increase decoding recursion.
`tests/differential.rs` proves it byte-compatible against a fully prost-generated oracle (`tests/oracle/`, regenerated
and CI-checked together with the main codegen).

## Children are untrusted

A parent must treat every frame from a (possibly compromised) child as
untrusted input: conversions from proto to Rust are fallible by design,
decoding enforces a per-frame decode budget and validates every arena index,
and nothing in this crate panics on malformed wire data.

Decode protocol types through `decode_frame` or `FrameReader::read`.
These functions manage the per-frame allocation budget automatically.
Direct `Message::decode` calls on these types fail if they allocate payload storage without a frame budget.

Frames are capped at 256 MiB, with a separate fixed 1 GiB budget for cumulative decoded allocation requests.
This budget is independent of the session's `max_memory`.
Compact messages can require much more memory when decoded.
The budget covers arena slots, repeated-field capacity, strings, byte buffers, boxed payloads and BigInt storage.
Temporary protobuf buffers and conversions into domain nodes share this budget.
Child references include an allowance for host container storage; shared sub-objects are encoded once.
Growth charges the full replacement allocation, with no refunds for discarded payloads.
A frame can therefore exceed the budget even if its final decoded value occupies less than 1 GiB.
The receiver checks the budget before allocating payload storage.

The wire buffer, bounded stack and error storage, allocator metadata and subsequent host conversions are not counted.
Each concurrent decode has its own budget; this is not a process-memory limit.
See `DEFAULT_MAX_DECODE_BYTES` in `src/frame.rs` for the accounting contract.
The browser component uses separate decoded-value estimates for WIT arenas, with the same 1 GiB ceiling.
Those checks do not account for all allocations made by the component ABI or JavaScript conversion.

Invalid dates, timedeltas, exception names and other semantic values are rejected after parsing each protobuf node.
The browser component validates semantic values while converting WIT arenas.
A parent receiving an invalid frame discards the worker with a protocol error.
A worker receiving such a malformed request reports `RuntimeError("protocol violation: malformed request: ...")`
and keeps the session.

## Worker state machine

The `worker` cargo feature (off by default) adds the `worker` module: the
transport-agnostic child state machine, shared by the native `monty subprocess`
worker and the wasm worker. It links the `monty` interpreter, so only
worker-side crates enable it.

An external `FunctionCall` with `allow_eager_await = true` permits the parent to await a coroutine before replying.
The parent sends its value or exception in `ResumeFutures`, with exactly one result matching the call ID.
The worker creates a settled awaitable and continues, avoiding a separate `ResolveFutures` suspension.
Synchronous returns still use `ResumeCall`; parents may also ignore the hint and register a pending future as before.
Older workers omit the flag, which defaults to false, so newer parents retain the existing reply sequence.

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
