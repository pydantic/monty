# `copy`

Monty implements `copy.copy` and `copy.deepcopy`. Both walk the object graph
themselves, dispatching on the type in front of them, because there is no
pickle protocol behind them: CPython's `copy` reaches everything it does not
special-case through `__reduce_ex__`, and Monty has no `pickle`, `copyreg` or
`__reduce__`.

## Missing from the module

- `copy.replace()` (3.13's `__replace__` protocol).
- `copy.Error` / `copy.error`. Nothing in Monty raises it — the objects that
  cannot be copied raise the `TypeError` CPython's pickler raises for them.

Both are absent from the module namespace rather than stubbed, so they fail
type checking as well as raising `AttributeError` at runtime.

## The reducer protocol is ignored

`__copy__` and `__deepcopy__` are honoured on class instances. Everything else
CPython consults is not: `__reduce__`, `__reduce_ex__`, `__getstate__`,
`__setstate__`, `__getnewargs__` and `copyreg.dispatch_table` are never called.

For an ordinary class the result is the same as CPython's — a new instance of
the same class, built without calling `__init__`, holding a copied `__dict__`.
A class that customises pickling gets a plain attribute copy instead of the
representation it asked for.

A `__copy__` or `__deepcopy__` that calls a host function (an external
function, or anything reaching the filesystem) raises `NotImplementedError`;
copying runs synchronously and cannot suspend, the same limit `sorted(key=...)`
has.

## Types that cannot be copied

`copy.copy` and `copy.deepcopy` raise `TypeError: cannot pickle '<type>'
object` for modules, open files, coroutines, futures, external functions,
iterators of every kind, dict views, and host class instances.

A host class instance is refused because its identity belongs to the host: a
copy would carry the same instance id, so any attribute outside the eagerly
sent set would still resolve through the object it was supposed to be detached
from. CPython, where the same object is an ordinary instance, copies it.

CPython raises the same error for most of these, but not all: it copies
iterators whose type is picklable, so `copy.deepcopy(iter([1, 2]))` returns a
fresh iterator in CPython and raises here.

The type named in the message is Monty's, so it can differ from CPython's name
for the same object (see ./classes.md).

## Immutable values are shared rather than rebuilt

Both `copy.copy` and `copy.deepcopy` return the same object for `datetime`,
`date`, `time`, `timedelta`, `timezone`, `Path`, `slice` and exception
instances, where
CPython builds an equal new one through the pickle protocol. Nothing in Monty
can mutate these, so only `is` can tell the difference. `copy.copy` of a named
tuple *does* build a new object, matching CPython.

The cases where sharing is CPython's behaviour too — `str`, `bytes`, `int`,
`tuple` of immutables, `frozenset` under `copy.copy`, `range`, compiled
patterns, classes, functions — behave identically.

## Deep nesting is copied further than CPython manages

A `deepcopy` step charges one recursion level, so with the default
`max_recursion_depth` of 1000 a structure nested 999 deep still copies. CPython
stops at 498: its `copy` is written in Python and spends two frames per level
against the same limit of 1000.

The depth is counted in copy steps, not in levels of the source, so a shape
holding more than one container per level reaches the limit sooner — a list
inside a tuple inside a list costs two steps a level and stops at 499, where
CPython stops at 249.

Copying recurses on the native stack. For the most deeply nested dicts and
class instances that stack runs out before the recursion limit does; see the
recursion notes in ./resource_limits.md.

## The memo

`copy.deepcopy(x, memo)` accepts and populates a caller-supplied memo dict,
keyed by `id()` as CPython's is, and passes it to `__deepcopy__`. It does not
receive CPython's private `_keep_alive` entry (the list CPython stores under
`id(memo)` to pin the sources it has visited), so the memo holds one fewer
entry than CPython's would, and re-using one memo dict across separate
`deepcopy` calls is unsupported — within a single call, sources are pinned.

`deepcopy`'s third parameter, CPython's private `_nil` sentinel, is accepted
for signature compatibility and ignored.

Passing a `memo` that is neither a dict nor `None` raises
`TypeError: deepcopy() memo must be a dict or None, not <type>`, where CPython
fails later and less clearly (`AttributeError: 'int' object has no attribute 'get'`).

Passing the object being copied as its own memo — `copy.deepcopy(d, d)` — raises
`RuntimeError: dictionary changed size during iteration`, because recording the
copy grows the dict being walked. CPython also fails, with
`AttributeError: 'dict' object has no attribute 'append'` from its `_keep_alive`.

## `copy.copy` of a dict re-hashes its keys

CPython copies a dict's hash table, so `copy.copy(d)` never calls a key's
`__hash__`. Monty re-inserts each pair, so a key with a custom `__hash__` sees
it called again, and one that raises makes the copy fail where CPython's
succeeds. This is `dict.copy()`'s behaviour, which `copy` inherits — see
./builtins.md.
