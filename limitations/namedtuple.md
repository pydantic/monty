# Named tuples

Named tuples can be constructed with `collections.namedtuple` (see
[collections.md](collections.md)), and also enter the sandbox as
`sys.version_info` and as values passed in from the host via the
`MontyObject` API.

`typing.NamedTuple` remains a marker only; subscripting / `class`
inheritance does not produce a type (no `class` statement; see
[language.md](language.md)).

## Supported operations

- Construction via `collections.namedtuple`, including positional, keyword,
  and mixed calls, `rename`, and `defaults`.
- Indexing by integer: `nt[0]`. `IndexError` on out-of-range.
- Field access by name as an attribute: `nt.major`. `AttributeError` on
  unknown names.
- `len(nt)`, iteration (`for x in nt`).
- Equality: `nt == nt2` and `nt == (a, b, c)` — a named tuple equals a
  plain tuple with the same elements (matches CPython).
- Hashing: same hash as a plain tuple with the same elements; usable as
  a dict key or set element.
- `repr(nt)` — `Name(field1=v1, field2=v2, ...)` matching CPython.
- `bool(nt)` — `True` if non-empty, `False` if empty (tuple semantics).
- The inherited `tuple` surface: membership (`x in nt`), `nt.count(x)`,
  `nt.index(x[, start[, stop]])`, and lexicographic ordering (`< <= > >=`),
  including against a plain tuple in either direction and between two
  different namedtuple classes (the class name takes no part). Also slicing
  (`nt[1:3]`), concatenation (`nt + nt2`, `nt + (1,)`, `(1,) + nt`) and
  repetition (`nt * 3`, `3 * nt`), each producing a plain `tuple` as in
  CPython. These come from `tuple`, so they work on `sys.version_info` too.
- `nt.__getnewargs__()`. The shape follows CPython's split by origin: a
  `collections.namedtuple` instance returns its values flat (`(1, 2, 3)`),
  while a structseq wraps them one level deeper (`sys.version_info` gives
  `((3, 14, 0, 'final', 0),)`) because its `__new__` takes a single sequence.
- `type(p) is Point`, `type(p).__name__`, `Point.__qualname__` (always equal
  to `__name__`, as CPython sets it explicitly), the class attributes `_fields`
  and `_field_defaults`, and the methods `_make`, `_replace`, `_asdict` (see
  collections.md for the divergences that remain). These require a `collections.namedtuple`
  class: `sys.version_info` and host-supplied named tuples model CPython
  *structseqs*, which expose none of them (`sys.version_info._fields` raises
  `AttributeError`, matching CPython).

## NOT supported

- Concatenating a named tuple with a `list` reports
  `TypeError: unsupported operand type(s) for +: 'namedtuple' and 'list'`
  where CPython says `can only concatenate tuple (not "list") to tuple`.
  This is Monty's existing message for plain tuples too (`(1,) + [2]` reports
  the same shape), not something specific to named tuples.
- Accessing a namedtuple method without calling it (`m = p._asdict`) raises
  `AttributeError` — the methods are call-only, not bound-method values.
  This is repo-wide, not namedtuple-specific: `[1].append`, `'a'.upper` and
  `{}.get` all raise the same way.
- A string subscript (`nt['x']`) raises `TypeError` as in CPython, but the
  message reads `tuple indices must be integers, not 'str'` where CPython
  says `tuple indices must be integers or slices, not str`. Plain tuples and
  lists word it the same way, so this is not specific to named tuples.
- Subclassing.
