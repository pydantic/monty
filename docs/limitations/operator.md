# `operator` and `_operator` modules

`_operator` is native, as in CPython, and `operator` re-exports it until the vendored `operator.py` arrives
([#1275](https://github.com/pydantic/monty/issues/1275)).

Available: `add`, `sub`, `mul`, `truediv`, `floordiv`, `mod`, `pow`, `matmul`, `neg`, `pos`, `abs`, `invert`, `and_`,
`or_`, `xor`, `lshift`, `rshift`, `index`, `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `not_`, `truth`, `is_`, `is_not`,
`contains`, `getitem`, `concat`, `countOf`, `indexOf`, `setitem`, the in-place forms (`iadd`, `isub`, `imul`,
`itruediv`, `ifloordiv`, `imod`, `ipow`, `iand`, `ior`, `ixor`, `ilshift`, `irshift`, `iconcat`), `call`, `itemgetter`,
`attrgetter` and `methodcaller`.

- **`length_hint`, `_compare_digest`, `inv`, `is_none`, `is_not_none`, `delitem` and `imatmul` are left out**, so they
    raise `AttributeError`.
    `delitem` waits on the `del` statement and `imatmul` on `@=`.
- **`operator` has no dunder aliases (`__add__` and so on) and no `__all__`.**
    The vendored `operator.py` will define them.
- **`copy.copy()` and `copy.deepcopy()` of an `itemgetter`, `attrgetter` or `methodcaller` return the same object.**
- **`attrgetter` reads attributes, so it cannot read a built-in method such as `str.upper`.**
    `methodcaller('upper')` works on a `str`.
