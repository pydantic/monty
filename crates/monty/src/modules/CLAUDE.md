# Adding / extending stdlib modules

Guidance for the Python stdlib modules implemented in this directory
(`re`, `json`, `math`, `datetime`, `unicodedata`, …). The overriding goal is
**CPython parity**: signatures, error messages, and error *ordering* must
match CPython 3.14 byte-for-byte. Verify every message against CPython with
the `/python-playground` skill before pinning it in a test.

## Declare parameters with `#[py_signature]`

Declare the Python parameter list on the Rust function. Use `bind_positional!` inside a method
dispatch arm that consumes raw positional values. Use a named signature struct when dispatchers share a binder. The [macro reference](../../../monty-macros/README.md) covers ownership, defaults and converters.

### Pick the `style` by how CPython implements the function

| CPython implementation                                                                                                                                                                            | `style`                                                       |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| pure-Python `def` (the `re` functions, `json.dumps`)                                                                                                                                              | `style = def`                                                 |
| Argument Clinic, positional-only (`math.pow`)                                                                                                                                                     | default — omit `style`                                        |
| Argument Clinic with keyword-capable params (`accumulate`, `os.stat`)                                                                                                                             | `style = c_named` — it shares `_PyArg_UnpackKeywords` wording |
| `PyArg_ParseTupleAndKeywords`, anonymous `function` errors                                                                                                                                        | `style = c`                                                   |
| same, with the name embedded (`timezone() missing …`)                                                                                                                                             | `style = c_named`                                             |
| `PyArg_UnpackTuple` (positional-only, `min..max` arity, kwargs rejected wholesale with `takes no keyword arguments`)                                                                              | `style = unpack`                                              |
| `tp_vectorcall` fast path in front of a clinic parser (`int`, `str`: kwarg-free overflow says `int expected at most 2 arguments, got 3`, with kwargs `int() takes at most 2 arguments (3 given)`) | default style + `at_most_total, vectorcall`                   |

The style controls wording *and* ordering (e.g. C families report leftover
kwargs last; clinic/def bind fully before any conversion). When unsure, probe
CPython: call the function with a bad type + a bogus kwarg, too many args,
etc., and match the observed messages.

### Parameters and ownership

`re.search` uses `#[py_signature("search(pattern, string, flags=0)", style = def)]`.
Each field holds a Python value. The body applies the conversions in the same order as CPython.
`itertools.accumulate` declares `accumulate(iterable, func=None, *, initial=None)` with
`style = c_named`, so passing three positionals produces CPython's positional overflow error.

For C functions, use `str` for text that the body only reads. In `unicodedata.normalize`,
both `form` and `unistr` are extracted as strings before the body validates the normalization form.
Use `=...` when omission differs from an explicit `None`.

Borrow the argument struct to have the adapter release its fields on return.
When the body takes ownership by destructuring the argument struct, it must consume
or guard each heap reference on every path.

### Tests and docs

- Add behaviour tests to `crates/monty/test_cases/` (dual-run against
    CPython — asserts must pass on both engines; exact `==` messages, never
    `in`). Signature-error orderings belong in `args__macro_errors.py`.
- Document every remaining CPython divergence in `./limitations/<module>.md`
    — including "obvious" ones.
- Run `make test-cases`, `make format-rs`, `make lint-rs` (and
    `make lint-py` for test changes) before finishing.
