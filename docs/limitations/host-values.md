# Values crossing the host boundary

Passing values between Monty and a host is not equivalent to calling another function in CPython.
These restrictions apply to inputs, host-function arguments and results, and a snippet's final result.
See the [Python](../quickstart/python.md#which-values-cross-the-boundary) and
[JavaScript](../quickstart/javascript.md#value-conversion) conversion tables for supported types.

## Copies and identity

Containers cross as copies; mutating a received list or dict does not mutate the sender's object.
Repeated references within one message share one copy, including across arguments or inputs.
Separate feeds and calls make separate copies.
[Host objects and sandbox instance proxies](classes.md#crossing-the-host-boundary-pydantic_monty-pydanticmonty)
have their own identity rules.

## Lossy outputs

Sandbox values without a host representation become strings rather than raising a conversion error.
Examples include sandbox-defined class objects, functions and compiled `re` patterns, which return their repr text.
Iterators return type placeholders, and a host-function proxy returned from the sandbox becomes its name.
Python and JavaScript hosts cannot distinguish these from ordinary sandbox strings with the same text.
Rust hosts can distinguish the corresponding `MontyNode` variants.
Sandbox-defined class *instances* instead return structured proxies;
see [classes](classes.md#crossing-the-host-boundary-pydantic_monty-pydanticmonty).

Builtin functions and type objects outside the data-type allowlist never resolve to the host's own objects.
Python receives a read-only [`MontyStdTypeProxy`][pydantic_monty.MontyStdTypeProxy] with `kind` and `name`,
JavaScript a `{ __monty_type__, value }` marker.
The Python proxy re-enters the sandbox as the builtin it names, as does a JavaScript `Type` marker;
a JavaScript `BuiltinFunction` marker passed back becomes its repr string.
The allowlist is `type`, `object`, `bool`, `int`, `float`, `str`, `bytes`, `list`, `tuple`, `dict`, `set`,
`frozenset`, `range`, `slice`, `NoneType`, `ellipsis`, `NotImplementedType`, the `datetime` classes,
`collections.deque`, `re.Pattern`, `re.Match`, `types.GenericAlias`, `types.UnionType`,
`pathlib.PurePosixPath` and the exception classes.
So `open`, `functools.partial` and `type(iter([]))` reach a Python host as proxies, while `int` and `datetime.date`
are the host classes.
A host class outside the allowlist passed *in*, such as `functools.partial` or an `itertools` adaptor, is an
unmodelled class and enters as a host function.

A self-referential container replaces the cycle with a placeholder string such as `[...]`, `{...}`, `(...)` or `...`.
Rust receives a `Cycle` node, which cannot be sent back as an input.
Cyclic host inputs are rejected: Python raises [`MontyRuntimeError`][pydantic_monty.MontyRuntimeError]
wrapping `ValueError('Circular reference detected')`; JavaScript throws `TypeError`.

Exporting sandbox values counts against `max_recursion_depth` (1000 by default, shared with the call stack).
Values below the remaining depth become the string `<deeply nested>` (`Repr` nodes in Rust).
The turn completes and the session remains usable; the wire format itself has no nesting limit.

## Host-function proxies

A host-function proxy dispatches by name against the current feed's `external_lookup`, not to a fixed callable.
Replacing an entry changes the target of every retained proxy for that name.
Replacing it with a non-callable makes calls raise `TypeError`.

Non-callable values resolved through `external_lookup` are cached in the sandbox namespace.
Later host-side changes to that entry are not observed by reads of the cached value.
Only undefined names trigger lookup: an entry named `len`, for example, does not override the builtin.
Use `inputs` to bind such a name explicitly.

## Conversion failures

An unsupported host-function or `os` callback result raises a catchable `TypeError` inside the sandbox.
A cyclic result raises a catchable `ValueError` from Python hosts, or `TypeError` from JavaScript hosts.
An uncaught conversion exception ends the feed as a runtime error.
Values supplied through `inputs` or `external_lookup` instead fail host-side;
see [Python error handling](../quickstart/python.md#errors).

## Message size

A value and its message envelope must fit within 256 MiB.
An oversized input, result or host-function payload fails the call without killing the worker.
If arguments make a suspension announcement too large, the feed ends with a host-visible `RuntimeError`
that sandboxed code cannot catch.
[Session dumps](../snapshots.md#storing-and-restoring) have the same size cap.

On protobuf transports, the receiver also limits cumulative decoded allocation requests to 1 GiB per frame.
A message can exceed this budget even if its final decoded values occupy less than 1 GiB.
A parent receiving such a frame discards the worker with a protocol error.
The browser component separately limits its WIT value arenas to an estimated 1 GiB.
Protocol validation and accounting are described in the
[`monty-proto` README](https://github.com/pydantic/monty/blob/main/crates/monty-proto/README.md#children-are-untrusted).
