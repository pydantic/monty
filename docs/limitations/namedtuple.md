# Named tuples

Named tuples can be constructed with `collections.namedtuple` (see
[collections.md](collections.md)), and also enter the sandbox as
`sys.version_info` and as values passed in from the host via the `MontyObject`
API. `typing.NamedTuple` is a marker only; subscripting it or inheriting from
it does not produce a type, since there is no class inheritance (see
[classes.md](classes.md)).

Instances behave as CPython named tuples: integer indexing, attribute access,
`len`/iteration/`bool`, equality and hashing against equivalent plain tuples,
and the inherited `tuple` surface (membership, `count`, `index`, ordering
against plain tuples and other namedtuple classes alike, slicing,
concatenation, and repetition, each producing a plain `tuple`). `_fields`,
`_field_defaults`, `_make`, `_replace` and `_asdict` require a
`collections.namedtuple` class: `sys.version_info` and host-supplied named
tuples model CPython *structseqs*, which expose none of them
(`sys.version_info._fields` raises `AttributeError`, as in CPython).

## Divergences

- **Concatenating with a `list`** reports `TypeError: unsupported operand
  type(s) for +: 'namedtuple' and 'list'` where CPython says `can only
  concatenate tuple (not "list") to tuple`. Monty's plain tuples word it the
  same way, so this is not namedtuple-specific.
- **A string subscript** (`nt['x']`) raises `TypeError` as in CPython, but reads
  `tuple indices must be integers, not 'str'` vs CPython's `... or slices, not
  str`. Plain tuples and lists word it the same way.
- **Accessing a method without calling it** (`m = p._asdict`) raises
  `AttributeError`: methods are call-only, not bound-method values. Repo-wide,
  `[1].append`, `'a'.upper` and `{}.get` all do the same.
- **Subclassing** is unsupported (see [classes.md](classes.md)).
