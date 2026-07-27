# `collections` module

`collections.deque`, `collections.namedtuple`, `collections.defaultdict`, and
`collections.Counter` are implemented. All are feature-complete against CPython
3.14 subject to the divergences below.

## Implemented

`deque`, with `append`, `appendleft`, `clear`, `copy`, `count`, `extend`,
`extendleft`, `index`, `insert`, `pop`, `popleft`, `remove`, `reverse`,
`rotate`, and the read-only `maxlen` attribute.

`namedtuple(typename, field_names, *, rename=False, defaults=None, module=None)`,
returning a callable class whose instances are named tuples (see
[namedtuple.md](namedtuple.md)). Supports the string and iterable field-name
forms, `rename`, `defaults`, the class attributes `_fields` / `_field_defaults`,
and the methods `_make` / `_replace` / `_asdict`. Field-name validation matches
CPython's messages exactly.

### `namedtuple` divergences

- **`repr(Point)` is `<class 'Point'>`**, where CPython gives
  `<class '__main__.Point'>`. This is the repo-wide unqualified-class-name
  pattern (user `class` objects render the same way), not specific to
  namedtuple. Instance repr (`Point(x=1, y=2)`) matches CPython.
- Construction/`_make`/`_replace` error *messages* match CPython 3.14 (e.g.
  `Point.__new__() missing 1 required positional argument: 'y'`).

`defaultdict` and `Counter` are exposed as the *type objects* (like `deque`), so
`type(d) is defaultdict`, `isinstance(c, Counter)` and the type-level
classmethods (`defaultdict.fromkeys`, the deliberately-disabled
`Counter.fromkeys`) all behave as in CPython.

`defaultdict(default_factory=None, /, *args, **kwargs)`, a `dict` subclass that
invokes `default_factory` on a missing-key access (via `__missing__`). Supports
the `default_factory` attribute (readable and writable), `_`-free dict methods,
`copy()` (preserving the factory), `repr`, `isinstance(d, dict)`, and equality
with a plain dict.

### `defaultdict` divergences

- **A `default_factory` cannot call an external or `os` function.** Accessing a
  missing key raises `NotImplementedError` if the factory tries to. Plain
  factories (`int`, `list`, `lambda`, ordinary functions) work. This restriction
  applies to every callback Monty invokes mid-expression — the `key=` of
  `sorted`/`min`/`max`, `map`, `filter`, and `__repr__`/`__str__` — so it is not
  specific to defaultdict.
- **Crosses the host boundary as a plain `dict`** — the host receives the keys
  and values, but nothing marks it as a defaultdict, and `type(defaultdict)`
  maps to the host `dict` type. The `default_factory` itself can never cross:
  it is a function, and the host value format has no way to represent one.
  Sending the dict back into Monty yields a plain `dict`.
- **KeyError text for non-string keys** follows Monty's existing dict behaviour,
  which already differs from CPython for tuple keys — not specific to
  defaultdict.

`Counter(iterable_or_mapping=None, /, **kwargs)`, a `dict` subclass where a
missing key reads as `0` (without inserting). Supports `most_common`,
`elements`, `total`, `update`/`subtract`, the arithmetic operators (`+ - & |`
and unary `+ -`, all keeping only positive counts), and `repr` in count order.

### `Counter` divergences

- **`elements()` returns a list**, not CPython's lazy iterator. The values and
  their order match; the difference shows up only if you rely on laziness —
  the whole sequence is built up front, so a Counter with very large counts
  can exhaust the memory limit where CPython would stream.
- **Crosses the host boundary as a plain `dict`** — the host receives the keys
  and counts, but nothing marks it as a Counter, and sending it back into Monty
  yields a plain `dict`.
- **Mixing `float` and big-int counts raises `TypeError`.**
  `Counter({'a': 3.5}) + Counter({'a': 2**70})`, and a `total()` over such a
  mix, raise `TypeError: unsupported operand type(s) for +: 'float' and 'int'`.
  Floats alone and big ints alone both work. Monty rejects the same mix outside
  Counter (`3.5 + 2**70` raises identically), so this is a general arithmetic
  limitation rather than a Counter one.
- **In-place `&=` against a non-mapping list borrows Monty's list-index
  wording.** Like CPython, `c &= other` subscripts `other[elem]`, so a
  non-empty `c &= [1, 2]` raises `TypeError` — but the message is Monty's
  `list indices must be integers, not 'str'` rather than CPython's
  `list indices must be integers or slices, not str`. This is the general
  list-subscript wording divergence, not a Counter one, and only surfaces for
  this unusual operand. Mapping operands (`c &= {'a': 1}`) match CPython
  exactly, including the `KeyError` for a key missing from a plain dict.

## Not implemented

`OrderedDict`, `ChainMap`, `UserDict`,
`UserList`, `UserString`. Importing one raises
`ImportError: cannot import name 'OrderedDict' from 'collections' (unknown location)`;
reaching it as an attribute raises
`AttributeError: 'module' object has no attribute 'OrderedDict'`.

The `collections.abc` submodule is not importable
(`ModuleNotFoundError: No module named 'collections.abc'`).

Type checking agrees: the custom stub
(`crates/monty-typeshed/custom/collections/__init__.pyi`) is narrowed to the
four implemented names, so `from collections import OrderedDict` is a type
error rather than something that type-checks and then fails at runtime.
`crates/monty/tests/collections.rs` pins the two sides together.

## Behavioural notes

### Qualified vs bare type names

Monty stores one name per type. CPython does not: it picks between a qualified
name (`collections.defaultdict`) and a bare one (`defaultdict`) *per surface*,
and which surfaces get which depends on whether the type is written in C or in
Python. So a single name cannot match CPython everywhere, and each type is
named for whichever spelling CPython uses most:

| | name Monty stores | what diverges |
|---|---|---|
| `deque` | `collections.deque` | only `__name__` (CPython gives `'deque'`) |
| `defaultdict` | `collections.defaultdict` | only `__name__` (CPython gives `'defaultdict'`) |
| `Counter` | `Counter` | only `repr(Counter)` and the `cannot use ...` clause of the unhashable message (CPython qualifies both) |

`deque` and `defaultdict` are C types in CPython, so it spells them qualified in
`repr(T)`, in the unhashable message, and in every error message that names a
type (`unsupported operand type(s)`, `object is not callable`, `object has no
attribute`) — Monty matches all of those and pays only on `__name__`. `Counter`
is a *Python-level* class in CPython, so those same error messages give the bare
`'Counter'`, and Monty's bare name matches them; only `repr(Counter)` and the
unhashable message qualify it.

CPython even mixes both spellings inside one message —
`cannot use 'collections.Counter' as a dict key (unhashable type: 'Counter')` —
which nothing with a single name can reproduce.

So code that matches on a type name — parsing an error message, or comparing
`__name__` against a literal — may need to accept either spelling. The same
applies to `datetime.datetime`, `re.Pattern` and `_io.TextIOWrapper`.

- **Mutation during iteration is not detected through `enumerate`, `zip`, `map`,
  `filter` or `reversed`.** Those builtins are eager in Monty — they return a
  `list`, not a lazy iterator (see [builtins.md](builtins.md)) — so the deque has
  already been fully read by the time the loop body runs, and a mutation inside
  the body cannot be observed. `for x in d` (and explicit `iter()`/`next()`)
  detect mutation exactly as CPython does, including a mutation on the final
  iteration.

- **`del d[i]` is not supported** — the `del` statement is unimplemented across
  Monty (see [language.md](language.md)), so this is not a deque limitation.
  Note it fails at *compile* time, not when the statement is reached: a `del`
  anywhere in the source — even inside a function that is never called — raises
  `NotImplementedError: The monty syntax parser does not yet support the 'del'
  statement` before any of the program runs. Use `remove()`, `pop()`, or
  `popleft()`.

- **Subclassing is not possible.** `class Q(deque)` raises
  `NotImplementedError: The monty syntax parser does not yet support class
  inheritance and metaclasses` (see [classes.md](classes.md)).

- **A deque returned to the host** (the Python / JS APIs) arrives as a plain
  list. Its items keep their own types, but two things do not survive: the
  `maxlen` bound, and the fact that it was a deque at all. Sending the list
  back into Monty yields a `list`, not a deque. This mirrors how defaultdict
  and Counter arrive as plain dicts.

### Divergences shared with `list`

Deque follows Monty's existing sequence behavior, which departs from CPython
here:

- **`d * 1.5`** raises `TypeError: unsupported operand type(s) for *:
  'collections.deque' and 'float'`, where CPython says
  `can't multiply sequence by non-int of type 'float'`.

- **Repeat counts in `[2**63, 2**64)` are accepted** where CPython rejects them.
  Monty's sequence repeat count is a `usize` (max `2**64 - 1`); CPython's is a
  C `ssize_t` (max `2**63 - 1`). This is only observable for a bounded deque,
  whose result truncates to `maxlen`: `deque([1, 2], maxlen=2) * 2**63` yields
  `deque([1, 2], maxlen=2)` in Monty but raises `OverflowError: cannot fit 'int'
  into an index-sized integer` in CPython. A count of `2**64` or more raises that
  same `OverflowError` in both. (Unbounded repetition at these counts fails in
  both — Monty via a resource/memory limit, CPython via `OverflowError`.)

Note `list.index()` has its own divergence that `deque.index()` does *not*
share: a non-integer bound raises
`TypeError: 'str' object cannot be interpreted as an integer` rather than
CPython's `slice indices must be integers or have an __index__ method`.
`deque.index()` matches CPython on the message, on rejecting an explicit `None`
bound, and on clamping out-of-`i64`-range bounds by sign.

Note `d += <any iterable>` *does* work (it is `extend`, as in CPython), even
though `list`'s `+=` still accepts only another list.
