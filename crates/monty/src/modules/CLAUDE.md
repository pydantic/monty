# Adding / extending stdlib modules

Guidance for the Python stdlib modules implemented in this directory
(`re`, `json`, `math`, `datetime`, `unicodedata`, …). The overriding goal is
**CPython parity**: signatures, error messages, and error *ordering* must
match CPython 3.14 byte-for-byte. Verify every message against CPython with
the `/python-playground` skill before pinning it in a test.

## Always use `#[derive(FromArgs)]` for function parameters

Any function beyond the trivial 0/1/2-positional shapes (covered by
`ArgValues::check_zero_args` / `get_one_arg` / `get_two_args` /
`get_zero_one_arg` / `into_pos_only`) MUST define its parameters with a
`#[derive(FromArgs)]` struct. Never hand-roll `args.into_parts()` loops —
they leak refcounts and diverge from CPython's wording. The derive emits a
static param spec driven by the runtime binder
(`crate::args::bind_native`), which handles dispatch, arity errors, kwarg
matching, duplicate/conflict detection, and refcount cleanup mechanically.

Full attribute reference: `crates/monty-macros/README.md` (including the
CPython-family table and the `at_most_total` litmus test).

### 1. Pick the `style` by how CPython implements the function

| CPython implementation | `style` |
|---|---|
| pure-Python `def` (the `re` functions, `json.dumps`) | `style = def` |
| Argument Clinic (most modern C builtins/methods) | default — omit `style` |
| `PyArg_ParseTupleAndKeywords`, anonymous `function` errors | `style = c` |
| same, with the name embedded (`timezone() missing …`) | `style = c_named` |
| `PyArg_UnpackTuple` (positional-only, `min..max` arity) | `style = unpack` |

The style controls wording *and* ordering (e.g. C families report leftover
kwargs last; clinic/def bind fully before any conversion). When unsure, probe
CPython: call the function with a bad type + a bogus kwarg, too many args,
etc., and match the observed messages.

### 2. Typical shapes (real examples from this directory)

A pure-Python `def` — fields stay raw `Value` because CPython `def` binding
never type-checks; coerce in the body (`re.rs`):

```rust
#[derive(FromArgs)]
#[from_args(name = "search", style = def)]
struct ReSearchArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}
```

A clinic-style function with keyword-only params (`math.rs`):

```rust
#[derive(FromArgs)]
#[from_args(name = "isclose")]
struct IscloseArgs {
    a: Value,
    b: Value,
    #[from_args(kw_only, default = Value::Float(1e-9))]
    rel_tol: Value,
    #[from_args(kw_only, default = Value::Float(0.0))]
    abs_tol: Value,
}
```

A positional-only C function with typed extraction (`unicodedata.rs`) —
`StrArg` borrows the text zero-copy. Note the form *name* is validated in the
body (`NormForm::parse`), not during extraction: CPython type-checks every
argument before the body rejects a bad value, so value checks must not run
inside `FromValue`:

```rust
#[derive(FromArgs)]
#[from_args(name = "normalize", style = unpack, bad_arg)]
struct NormalizeArgs {
    #[from_args(pos_only)]
    form: StrArg,
    #[from_args(pos_only)]
    unistr: StrArg,
}
```

Calling and consuming — sketch; see `unicodedata.rs` for the real body (note
`defer_drop!` for fields that hold heap refs):

```rust
fn call_normalize(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let NormalizeArgs { form, unistr } = NormalizeArgs::from_args(args, vm)?;
    defer_drop!(unistr, vm);
    let normalized = form.apply(unistr.as_str(vm)); // sketch — do the work here
    // ... allocate and return the result; the guard releases `unistr`
}
```

### 3. Field rules

- Declaration order = Python signature order:
  `[pos_only…] [pos_or_keyword…] [varargs] [kw_only…] [varkwargs]`, required
  fields before defaulted ones in the positional region.
- **Typed fields** (`i64`, `i32`, `bool`, `StrArg`, `Option<T>`, custom
  `FromValue` impls) are only for functions implemented in C in CPython —
  pair with `bad_arg` / `bad_arg_named` when CPython uses
  `_PyArg_BadArgument` wording (`f() argument 1 must be str, not int`).
- **`style = def` fields must be raw `Value`** (coerce in the body so the
  error message matches what CPython's function body raises).
- Prefer `StrArg` over `String` for str params the function only reads — it
  validates without copying and lends `&str` via `.as_str(vm)`.
- Field names that aren't single ASCII chars need a `StaticStrings` variant
  (in `crate::intern`) for kwarg matching — add one, or point
  `static_string = "ExistingVariant"` at an existing one.
- `Value` / `StrArg` / `Option<Value>` fields hold heap references: bind
  them with `defer_drop!` in the body, or otherwise guarantee
  `drop_with_heap` on every path.
- `varargs` fields must be `Vec<Value>`; convert elements in the body.

### 4. Tests and docs

- Add behaviour tests to `crates/monty/test_cases/` (dual-run against
  CPython — asserts must pass on both engines; exact `==` messages, never
  `in`). Signature-error orderings belong in `args__macro_errors.py`.
- Document every remaining CPython divergence in `./limitations/<module>.md`
  — including "obvious" ones.
- Run `make test-cases`, `make format-rs`, `make lint-rs` (and
  `make lint-py` for test changes) before finishing.
