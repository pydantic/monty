# Follow-up plan: preserve list identity for `+=`

## Summary

The subscript augmented-assignment fix in `fix-subscript-augassign` makes code like:

```python
totals = {'photo': 1}
rtype = 'photo'
likes = 2
totals[rtype] += likes
```

parse and execute correctly.

However, there is a broader CPython-compatibility gap around **list identity preservation** for
in-place addition. In CPython, list `+=` mutates the existing list object. In Monty today, the
behavior appears to fall back to rebinding in some cases, which breaks aliasing and prevents the
full immutable-container edge case from matching CPython.

## What works now

These Python patterns are covered by test cases and should work after the current fix:

```python
totals = {'photo': 1}
rtype = 'photo'
likes = 2
totals[rtype] += likes
assert totals == {'photo': 3}
```

```python
lst = [1, 2, 3]
index = 1
lst[index] += 5
assert lst == [1, 7, 3]
```

The current test coverage also checks:

- the index expression is evaluated once
- missing dict keys raise `KeyError`
- failed operations do not overwrite the original dict item
- out-of-range list indices raise `IndexError`

## What should be fixed in the follow-up

The next bug to fix is true in-place list semantics, especially alias preservation:

```python
x = [1]
y = x
x += [2]

# CPython:
assert x is y
assert y == [1, 2]
```

Once that works, this CPython behavior should also fall out naturally:

```python
t = ([1],)
try:
    t[0] += [2]
except TypeError as e:
    assert e.args == ("'tuple' object does not support item assignment",)
    assert t == ([1, 2],)
```

In CPython, the inner list is mutated first and then the tuple store fails because tuples are
immutable.

## Why Python test cases should drive the follow-up

This repo already has strong comparative testing at the Python source level:

- `crates/monty/test_cases/*.py` are executed against **both Monty and CPython**
- the harness lives in `crates/monty/tests/datatest_runner.rs`
- `make test-cases` is the main command for this comparison

That means we do **not** need a vendored copy of CPython's own test files to validate this work.
For this follow-up, the best regression tests are small, explicit Python files or additions to the
existing consolidated test files that are checked directly against live CPython behavior.

## Proposed test-first plan

### 1. Add a list identity regression test

Add to an existing consolidated Python test file, likely `crates/monty/test_cases/list__ops.py`:

```python
x = [1]
y = x
x += [2]
assert x is y, 'list += preserves identity'
assert y == [1, 2], 'list += mutates aliases'
```

This is the key root-cause test.

### 2. Add the immutable-container edge case

Add to `crates/monty/test_cases/tuple__ops.py`:

```python
t = ([1],)
try:
    t[0] += [2]
    assert False, 'tuple item augmented assignment should fail'
except TypeError as e:
    assert e.args == ("'tuple' object does not support item assignment",), 'tuple += error matches CPython'
    assert t == ([1, 2],), 'inner list mutation happens before tuple store fails'
```

This should remain a Python test case, not a Rust-only regression test, because the goal is exact
CPython-visible behavior.

### 3. Reproduce with `make test-cases`

Use the comparative harness first:

```bash
make test-cases
```

If you need to iterate on a smaller repro, also check a tiny snippet directly in CPython and Monty,
but the final proof should be the Python test-case suite.

### 4. Trace the runtime `+=` path

The expected implementation work is likely in:

- `crates/monty/src/bytecode/vm/binary.rs`
- `crates/monty/src/value.rs`
- `crates/monty/src/types/list.rs`

The goal is to ensure list `py_iadd`:

- mutates the existing heap object
- preserves object identity
- returns the signal that tells the VM not to fall back to regular `py_add`

### 5. Re-run Rust and Python quality gates

After the runtime change:

```bash
make format-rs
make lint-rs
make lint-py
make test-cases
```

## Success criteria

The follow-up is complete when all of these hold:

```python
x = [1]
y = x
x += [2]
assert x is y
assert y == [1, 2]
```

```python
t = ([1],)
try:
    t[0] += [2]
except TypeError:
    pass
assert t == ([1, 2],)
```

and the behavior passes through the normal comparative Python test-case harness.
