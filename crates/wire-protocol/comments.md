# `wire_protocol` — behaviour notes

`wire_protocol` exposes the Monty subprocess wire protocol
(`proto/monty/v1/monty.proto`) to Python as four codec functions
(`encode_parent_request`, `decode_parent_request`, `encode_child_event`,
`decode_child_event`) plus frozen message classes mirroring the `ParentRequest`
and `ChildEvent` oneof arms. It exists so a sandbox can be driven over an
arbitrary transport (WebSocket, HTTP, socket, Docker pipe). See
[`websocket_plan.md`](./websocket_plan.md) for usage and the full footgun list;
this file records how the package behaves where it might surprise you.

## Not a transport

The package only encodes/decodes bytes. It opens no sockets, adds no framing,
performs no version handshake on its own, and does not drive a worker. The
caller owns the transport, the framing (for byte streams), the version check,
and the drive loop.

## Framing

`encode_*` returns a bare protobuf message with **no** length prefix (unlike
`monty_proto::write_frame`, which prepends a 4-byte LE length). Message-oriented
transports (WebSocket, HTTP) need nothing more; byte streams must add the prefix
themselves.

## Value model

- Values are native Python objects, converted by the same code path as
  `pydantic_monty` (it shares the `convert`/`dataclass` modules). The supported
  set and its CPython divergences are identical to `pydantic_monty`'s.
- **Dataclasses do not round-trip to their original type.** The codec is
  stateless (no dataclass registry), so a decoded dataclass is an
  `UnknownDataclass` carrying the fields, not an instance of the source class.
  The wire `type_id` (`id(type(obj))`) is preserved but is meaningless outside
  the process that produced it.
- Value validation happens at **encode** time, not construction in every case:
  unsupported types raise `TypeError` when the containing message is built
  (constructors convert eagerly), but cross-message structural errors surface
  from `encode_*`/`decode_*`.

## Limits dict

`StartSession(limits=...)` takes a mapping keyed by the **proto** field names and
units — `max_allocations`, `max_duration_micros`, `max_memory_bytes`,
`gc_interval`, `max_recursion_depth` — *not* `pydantic_monty`'s
`max_duration_secs`-style dict. Unknown keys are ignored; absent/`None` values
mean "unlimited" (except recursion depth, which the child defaults).

## Message classes

- Constructors are keyword-heavy and frozen; instances are value-comparable
  (`==`) and `repr`-able but not hashable.
- Every `ChildEvent` arm carries `total_execution_micros` and
  `max_duration_micros` (defaulting to `0` / `None`), since those live on the
  envelope, not the arm. Hand-built events leave them at the defaults.
- `Ok` is the Python name of the acknowledgement event (the Rust type is
  `OkEvent`).
- `RaisedException` carries `exc_type` (must be a name Monty knows, else
  `ValueError`), an optional `message`, and a `StackFrame` list.
  `from_exception` captures type + `str()` only — **no traceback**.
- `ResumeNameLookup` distinguishes "resolved to `None`" from "undefined" via
  `is_defined`; `value` alone is ambiguous.

## Version field

`StartSession.monty_version` defaults to the package `__version__`. The package does
not enforce it; the receiving `monty --subprocess` child does, replying
`FatalError` and exiting non-zero on mismatch. A custom server built on this
package must perform the equivalent check itself.

## Errors

All decode failures and invalid-field errors raise `ValueError` (prefixed
`invalid wire message:` for proto/conversion failures); type-mismatched
encode inputs raise `TypeError`. There is no dedicated exception hierarchy.
