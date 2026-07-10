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
  `assert 0` → `assert 0`.
- Chained comparisons (`assert 1 < 2 > 3`), `not` expressions, and boolean
  operators evaluate to a `bool` first, so their message degrades to
  `assert False`.
- `assert x % n == k` also degrades to `assert False`: Monty internally fuses
  `% ... ==` into a single comparison with no separate lhs/rhs to show, even
  though the source contains a syntactic `==`.

## `assert test, msg` appends the detail on a new line

- `assert 1 == 2, 'my message'` raises
  `AssertionError('my message\nassert 1 == 2')`; CPython raises
  `AssertionError('my message')`.
- Consequently `e.args[0]` contains the combined string, not the original
  message object. Non-`str` messages are rendered with `str()`
  (`assert False, 123` → `123\nassert False`); CPython stores the object
  itself in `e.args`.
- The message expression is still only evaluated on failure, as in CPython.

## Formatting edge cases

- Each operand's repr is truncated to 120 characters with a `...` suffix.
- If an operand's `__repr__` (or an explicit message's `__str__`) raises, that
  part is dropped rather than replacing the `AssertionError`: a bare assert
  falls back to a message-less `AssertionError`, an explicit-message assert
  keeps whichever of message/detail rendered successfully.

## Opt-out for embedders

The Rust API can restore CPython's plain `AssertionError` behavior at compile
time via `MontyRun::new_with_options(..., CompileOptions { assert_messages:
false })` — this is how Monty's own CPython-parity test harness runs. The
Python and JavaScript packages always compile with messages on.
