# `collections` module

`deque`, `namedtuple`, `defaultdict`, and `Counter` are implemented and
feature-complete against CPython 3.14 subject to the divergences below. The
exact supported surface is pinned by the custom typeshed stub
(`crates/monty-typeshed/custom/collections/__init__.pyi`) and
`crates/monty/tests/collections.rs`. `defaultdict` and `Counter` are exposed as
the type objects themselves (like `deque`), so `type(d) is defaultdict` and
`isinstance(c, Counter)` hold.

## Not implemented

`OrderedDict`, `ChainMap`, `UserDict`, `UserList`, `UserString`, and the
`collections.abc` submodule. Importing one raises `ImportError: cannot import
name 'OrderedDict' from 'collections' (unknown location)` (or `AttributeError`
as an attribute); `import collections.abc` raises `ModuleNotFoundError`. The
narrowed typeshed stub makes `from collections import OrderedDict` a type error
too, rather than something that type-checks and then fails at runtime.

## `deque`

- **`d * 1.5`** raises `TypeError: unsupported operand type(s) for *:
  'collections.deque' and 'float'`, where CPython says `can't multiply sequence
  by non-int of type 'float'`. (Shared with `list`.)
- **Repeat counts in `[2**63, 2**64)` are accepted** where CPython rejects them
  — Monty's repeat count is a `usize`, CPython's a C `ssize_t`. Only observable
  for a bounded deque, whose result truncates to `maxlen`: `deque([1, 2],
  maxlen=2) * 2**63` yields `deque([1, 2], maxlen=2)` in Monty but `OverflowError`
  in CPython. `2**64` or more raises `OverflowError` in both.
- **Returned to the host as a plain `list`** — the `maxlen` bound and the
  deque-ness are lost; sending it back yields a `list`. (Same as
  defaultdict/Counter, which arrive as plain dicts.)
- **Mutation during iteration is not detected through `enumerate`/`zip`/`map`/
  `filter`/`reversed`** — those are eager (see [builtins.md](builtins.md)), so
  the deque is fully read before the loop body runs. `for x in d` and explicit
  `iter()`/`next()` detect it exactly as CPython does.
- `d += <any iterable>` works (it is `extend`), even though `list`'s `+=` still
  accepts only another list.
- **Extending from an eager builtin loses the partial result when it raises** —
  `extend`/`extendleft`/`+=` append each item as the source yields it, so an
  iterator raising part-way leaves the earlier items in place, as in CPython.
  But `map`/`filter`/`zip`/`enumerate` are eager (see
  [builtins.md](builtins.md)), so `d += map(f, xs)` with a raising `f` raises
  before the extend begins and appends nothing, where CPython appends whatever
  was yielded first.

`del d[i]` and subclassing (`class Q(deque)`) both fail at *compile* time — the
`del` statement and class inheritance are unimplemented Monty-wide (see
[language.md](language.md) / [classes.md](classes.md)), not deque limitations.

## `namedtuple`

Field-name validation matches CPython's messages exactly; see
[namedtuple.md](namedtuple.md) for the tuple-inherited surface and its
divergences. `repr(Point)` is `<class 'Point'>` where CPython gives
`<class '__main__.Point'>` — the repo-wide unqualified-class-name pattern.

## `defaultdict`

- **A `default_factory` cannot call an external or `os` function** — a
  missing-key access then raises `NotImplementedError`. Plain factories (`int`,
  `list`, `lambda`, ordinary functions) work. This applies to every callback
  Monty invokes mid-expression (the `key=` of `sorted`/`min`/`max`, `map`,
  `filter`, `__repr__`), not just defaultdict.
- **Crosses the host boundary as a plain `dict`** — the `default_factory` is a
  function and cannot cross, so sending the dict back yields a plain `dict`.

## `Counter`

- **`elements()` returns a list**, not CPython's lazy iterator. The values and
  order match, but the whole sequence is built up front, so a very large count
  can hit the memory limit where CPython would stream.
- **Crosses the host boundary as a plain `dict`.**
- **Mixing `float` and big-int counts raises `TypeError`** (`Counter({'a': 3.5})
  + Counter({'a': 2**70})`, and `total()` over such a mix). This is Monty's
  general arithmetic limitation — `3.5 + 2**70` raises identically — not a
  Counter one.
- **In-place `c &= [list]`** (a non-mapping) subscripts `other[elem]` like
  CPython and raises `TypeError`, but with Monty's list-index wording (`list
  indices must be integers, not 'str'` vs CPython's `... or slices, not str`).
  Mapping operands (`c &= {'a': 1}`) match CPython exactly, including the
  `KeyError` for a key missing from a plain dict.

## Qualified vs bare type names

Monty stores one name per type; CPython picks a qualified (`collections.deque`)
or bare (`deque`) name *per surface*, depending on whether the type is written
in C or in Python — so no single name matches everywhere. Each type takes the
spelling CPython uses in the most places:

| type | Monty's name | what diverges |
|---|---|---|
| `deque` | `collections.deque` | only `__name__` (CPython: `'deque'`) |
| `defaultdict` | `collections.defaultdict` | only `__name__` (CPython: `'defaultdict'`) |
| `Counter` | `Counter` | only `repr(Counter)` and the `cannot use ...` clause of the unhashable message |

`deque`/`defaultdict` are C types, so CPython qualifies them in `repr(T)` and in
every type-naming error message (`unsupported operand type(s)`, `object is not
callable`, `object has no attribute`) — Monty matches all of those and pays only
on `__name__`. `Counter` is a Python-level class, so those messages give the
bare name, which Monty matches. Code that matches on a type name — parsing an
error message or comparing `__name__` — may need to accept either spelling, as
with `datetime.datetime`, `re.Pattern`, and `_io.TextIOWrapper`.
