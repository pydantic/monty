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
- Consequently chained comparisons (`assert 1 < 2 > 3`), `not` expressions,
  and boolean operators — which evaluate to a `bool` first — carry no message
  when they fail, matching CPython.

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
- The message expression is still only evaluated on failure, as in CPython.

## Formatting edge cases

- Each operand's repr is truncated to 120 characters with a `…` suffix.
- If an operand's `__repr__` (or an explicit message's `__str__`) raises, that
  part is dropped rather than replacing the `AssertionError`: a bare assert
  falls back to a message-less `AssertionError`, an explicit-message assert
  keeps whichever of message/detail rendered successfully.

## Opt-out for embedders

CPython's plain `AssertionError` behavior can be restored per session:

- Rust: pass `CompileOptions { assert_message_annotations: false }` to
  `MontyRun::new` or `MontyRepl::new`.
- Python: `pool.checkout(assert_message_annotations=False)`.
- JavaScript: `pool.checkout({ assertMessageAnnotations: false })` (both the
  native and wasm-worker pools).

Monty's Rust, Python, and JavaScript surfaces default to messages on. The
CPython compatibility worker ignores this option and always uses CPython's
plain `AssertionError` behavior.
