# `operator` and `_operator` modules

The implementation is a native `_operator` module, as in CPython. `operator` is a stand-in that re-exports the same
objects (`operator.add is _operator.add`) until the vendored `operator.py` arrives (#1275). `import _operator` works.

The function forms of the operators, `itemgetter`, `attrgetter` and `methodcaller` are available. The functions
run the operator syntax itself, so results, errors and user dunder methods match `a + b` and friends.

Available: `add`, `sub`, `mul`, `truediv`, `floordiv`, `mod`, `pow`, `matmul`, `neg`, `pos`, `abs`, `invert`,
`and_`, `or_`, `xor`, `lshift`, `rshift`, `index`, `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `not_`, `truth`, `is_`,
`is_not`, `contains`, `getitem`, `concat`, `countOf`, `indexOf`, `setitem`, the in-place forms (`iadd`, `isub`,
`imul`, `itruediv`, `ifloordiv`, `imod`, `ipow`, `iand`, `ior`, `ixor`, `ilshift`, `irshift`, `iconcat`), `call`,
`itemgetter`, `attrgetter` and `methodcaller`.

- **Left out, so they raise `AttributeError`:** `length_hint`, `_compare_digest`, `inv`, `is_none`, `is_not_none`,
    `delitem` (the sandbox has no `del` statement), `imatmul` (no `@=`), and, on `operator`, the dunder aliases
    (`__add__` and so on) and `__all__`, which the vendored `operator.py` will define.
- **`contains()` is `b in a`**, so the error wording for a non-container follows the `in` syntax and can differ
    from CPython's.
- **`itemgetter`, `attrgetter` and `methodcaller` objects** hash by identity and compare by identity.
    `copy.copy()` and `copy.deepcopy()` return the same object.
- **`attrgetter` and `methodcaller` look names up the way the sandbox does.** `attrgetter` reads attributes, so it
    cannot read a built-in method such as `str.upper`; `methodcaller` calls methods, so `methodcaller('upper')`
    works on a `str`.
- **User `__lt__` and the other rich comparisons** go through the same paths as the `<` syntax, so anything the
    syntax does not yet support is unsupported here too.
