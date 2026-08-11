# `unicodedata` module

Covers the widely-used Unicode Character Database queries. Implemented
functions mostly match CPython 3.14, subject to the behavioural notes below.

## Implemented

`category`, `name`, `lookup`, `combining`, `normalize`, `is_normalized`.

**Constants**: `unidata_version`.

## Not implemented

`decimal`, `digit`, `numeric` (numeric-value tables), `bidirectional`,
`east_asian_width`, `mirrored`, `decomposition`, and the `ucd_3_2_0` object.
These require Unicode property data not available in the lightweight backing
crates; accessing them raises `AttributeError`.

## Behavioural notes

- **Unicode version skew**: `unidata_version` reports `"16.0.0"` to match
  CPython 3.14, but the backing crates carry independent data tables whose
  Unicode versions may differ from each other and from 16.0 (e.g.
  `unicode-normalization` currently ships Unicode 17.0 tables). Results for
  code points assigned, renamed, or recategorised across those versions may
  therefore diverge from CPython. Long-established code points (ASCII, common
  Latin/Greek, CJK) are unaffected.
- `lookup` resolves character names and some aliases, but named sequences that
  CPython accepts (e.g. `"KEYCAP NUMBER SIGN"`) are not supported and raise
  `KeyError`.
- `lookup` requires a `str` argument; CPython also accepts `bytes`. A non-`str`
  argument raises `TypeError: "expected string, not <type>"` rather than
  CPython's bytes-oriented message.
- All implemented functions are positional-only, but keyword-argument error
  wording does not match CPython. CPython raises
  `unicodedata.<fn>() takes no keyword arguments`; Monty instead reports the
  generated positional arity or unexpected-keyword error.
