# `assert`

Monty deliberately diverges from CPython on `assert` failure messages: failed
asserts raise an `AssertionError` carrying a pytest-style introspected message,
so sandboxed code (and hosts feeding errors back to users/LLMs) can see the
values involved instead of a blank `AssertionError`.

## Bare `assert` carries a message (CPython raises an empty `AssertionError`)

- `assert 2 == 5` raises `AssertionError('assert 2 == 5')`; CPython raises
  `AssertionError()` with empty `str(e)` and empty `e.args`.
- The message is visible everywhere the exception is: `str(e)`, `e.args[0]`,
  tracebacks, and host-side error objects.
- Applies when the test is a single binary comparison with one of
  `==`, `!=`, `<`, `<=`, `>`, `>=`, `is`, `is not`, `in`, `not in`:
  both operands' `repr()`s are substituted. Operands are evaluated exactly
  once — side effects are not duplicated.
- Any other failing test shows the falsy value's repr instead:
  `assert []` → `assert []`, `assert None` → `assert None`,
  `assert 0` → `assert 0` — except `False` itself, which adds no information:
  `assert False` raises a plain message-less `AssertionError`, exactly like
  CPython.
- Chained comparisons (`assert 1 < 2 > 3`) and `not` expressions produce
  `False` when they fail, so they carry no introspected message. `and` and `or`
  return an operand rather than coercing it to `bool`, so they show that final
  falsy operand unless it is literally `False`: `assert 1 and []` shows
  `assert []`.

## `assert test, msg` appends the detail on a new line

- `assert 1 == 2, 'my message'` raises
  `AssertionError('my message\nassert 1 == 2')`; CPython raises
  `AssertionError('my message')`.
- Consequently `e.args[0]` contains the combined string, not the original
  message object. Non-`str` messages are rendered with `str()`
  (`assert [], 123` → `123\nassert []`); CPython stores the object
  itself in `e.args`.
- When the test value is literally `False` no detail is appended, so
  `assert False, 'msg'` raises `AssertionError('msg')` — the same as CPython
  apart from the `str()` rendering of non-`str` messages.
- A message that stringifies to the empty string is treated as absent, so only
  the detail is shown: `assert 1 == 2, ''` raises `AssertionError('assert 1 == 2')`
  (CPython raises `AssertionError('')`), and `assert False, ''` raises a
  message-less `AssertionError`.

## Formatting edge cases

- At most 120 bytes of each operand's repr are retained, cut on a character
  boundary. A truncated repr gets a three-byte `…` suffix in addition to that
  limit. The retained-byte limit is configurable per session; see "Opt-out for
  embedders" below.
- A failing assert calls `repr()` on its operands, which CPython never does:
  user `__repr__` side effects run. Rendering is streamed and stops at the
  truncation cap, so parts of a container beyond the cap are never repr'd —
  their `__repr__`s (and any side effects) don't run at all. The temporary repr
  buffer and its formatting loop are not charged to the `ResourceTracker`.
- If an operand's `__repr__` (or an explicit message's `__str__`) raises a
  catchable Python exception, that part is dropped: a bare assert falls back to
  a message-less `AssertionError`, while an explicit-message assert keeps
  whichever of message/detail rendered successfully. Terminal internal and
  resource errors propagate instead.

## Opt-out for embedders

Introspected annotations can be disabled per session, restoring CPython's empty
message for bare asserts. This does not remove Monty's other exception
constructor differences: with annotations disabled, an explicit assert message
must still be a string; see [exceptions](exceptions.md). The retained repr
length can also be customized (an int >= 1, in bytes; 0 means "off", not
"retain no bytes"):

- Rust: pass `CompileOptions { assert_message_annotations:
  AssertMessageAnnotations::Off }` (or `::MaxBytes(n)`, a `NonZeroU32`;
  `::from_max_bytes(n)` maps 0 to `Off`) to `MontyRun::new` or
  `MontyRepl::new`.
- Python: `pool.checkout(assert_message_annotations=False)` (or `=n`).
- JavaScript: `pool.checkout({ assertMessageAnnotations: false })` (or `: n`;
  both the native and wasm-worker pools).

Monty's Rust, Python, and JavaScript surfaces default to messages on. The
CPython compatibility worker ignores this option and always uses CPython's
plain `AssertionError` behavior.
