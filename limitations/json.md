# `json` module

Monty's `json` provides `loads`, `dumps`, and the `JSONDecodeError`
exception. Parsing is backed by `jiter`; serialization is a hand-written
encoder matching CPython byte-for-byte for the supported keyword set.

## What's NOT in the module

`json.JSONEncoder` and `json.JSONDecoder` classes are not implemented —
the `cls=` keyword is rejected. `json.load(fp)` and `json.dump(obj, fp)`
are not implemented (no file-object protocol).

## `json.loads(s, **kwargs)`

- Accepts `str` or `bytes` as input.
- **No keyword arguments are accepted.** Passing any of `cls`,
  `object_hook`, `parse_float`, `parse_int`, `parse_constant`, or
  `object_pairs_hook` raises `TypeError: ... unexpected keyword argument`.
- `NaN`, `Infinity`, `-Infinity` are *always* accepted (CPython requires
  `parse_constant` or accepts them by default — same result, no toggle).
- Nesting depth is capped at 200 levels; deeper inputs raise
  `json.JSONDecodeError`.
- JSON integers that would exceed Monty's BigInt digit limit are rejected
  with `ValueError` (matching CPython's `int_max_str_digits` behaviour)
  rather than `JSONDecodeError`.

## `json.dumps(obj, **kwargs)`

Supported kwargs: `indent`, `sort_keys`, `ensure_ascii`, `allow_nan`,
`separators`, `skipkeys` — matching CPython semantics.

Rejected with `TypeError` if passed:

- `cls` — custom encoder classes are not supported.
- `default` — fallback encoder callback is not supported. Non-serializable
  values raise `TypeError` instead of routing through a callback.
- `check_circular` — circular reference detection is always on.

## `JSONDecodeError`

Inherits from `ValueError` (catchable as `except ValueError:`). The class
qualified name is `json.JSONDecodeError`; `__name__` matches CPython.
Error messages use the same `line N column M (char K)` suffix as CPython
(counting characters, not bytes).

When the input ends inside an unclosed array or object (`'['`, `'{'`,
`'{"a"'`, …), Monty always reports `Expecting ',' delimiter`, where CPython
distinguishes what was expected at that point (`Expecting value`,
`Expecting property name enclosed in double quotes`,
`Expecting ':' delimiter`). Positions match; only the message text differs.

Inside the sandbox the exception is message-only: the `msg`, `doc`, `pos`,
`lineno` and `colno` attributes CPython sets are not available. When a
sandbox `JSONDecodeError` surfaces to the host (e.g. via `pydantic_monty`),
it is rebuilt as a real `json.JSONDecodeError` with all five attributes from
a structured payload attached at raise time — except that documents larger
than 64 KiB are not carried, in which case `doc` is `''`. A `JSONDecodeError`
raised manually inside the sandbox has no payload and surfaces as a plain
`ValueError` carrying the message.
