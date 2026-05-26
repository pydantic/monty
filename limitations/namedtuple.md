# Named tuples

Named tuples exist as a heap type but cannot be **constructed** by
sandboxed Python code:

- `collections.namedtuple` — `collections` is not importable.
- `typing.NamedTuple` — exists as a marker only; subscripting / `class`
  inheritance does not produce a type (no `class` statement; see
  [language.md](language.md)).
- There is no builtin `namedtuple` factory.

Named tuples enter the sandbox in two ways: as `sys.version_info`, and
as values passed in from the host via the `MontyObject` API.

## Supported operations

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

## NOT supported

- Slicing: `nt[1:3]` raises — `__getitem__` only accepts integer keys.
  (CPython returns a plain tuple for slices.)
- Lookup by string key: `nt["major"]` raises `TypeError: ... indices must
  be integers`. Use attribute access instead.
- Named-tuple methods from CPython: `._replace(**kw)`, `._asdict()`,
  `._make(iterable)`, `._fields`, `._field_defaults`, `._source`.
- Concatenation / multiplication: `nt + nt2`, `nt * 3` are not
  implemented (plain tuples support these; named tuples in Monty do not).
- Subclassing.
