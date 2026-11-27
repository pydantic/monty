# Adding / extending stdlib modules

Match CPython's supported call shapes and exception classes. Native signature errors display the Monty signature.

## Always use `#[derive(FromArgs)]` for function parameters

Any function beyond the trivial 0/1/2-positional shapes (covered by
`ArgValues::check_zero_args` / `get_one_arg` / `get_two_args` /
`get_zero_one_arg` / `into_pos_only`) MUST define its parameters with a
`#[derive(FromArgs)]` struct. Never hand-roll `args.into_parts()` loops —
the binder handles refcount cleanup on errors. The derive emits a
static param spec driven by the runtime binder
(`crate::args::bind_native`), which handles dispatch, arity errors, kwarg
matching, duplicate/conflict detection, and refcount cleanup mechanically.

See [the macro reference](../../../monty-macros/README.md) for parameter kinds and defaults.

### Typical shapes (real examples from this directory)

A pure-Python `def` — fields stay raw `Value` because CPython `def` binding
never type-checks; coerce in the body (`re.rs`):

```rust
#[derive(FromArgs)]
#[from_args(name = "search")]
struct ReSearchArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::Int(0))]
    flags: Value,
}
```

Two positional-or-keyword parameters and a keyword-only parameter from `itertools.rs`

```rust
#[derive(FromArgs)]
#[from_args(name = "accumulate")]
struct AccumulateArgs {
    #[from_args(static_string = "IterableArg")]
    iterable: Value,
    #[from_args(default = Value::None)]
    func: Value,
    #[from_args(kw_only, default = Value::None)]
    initial: Value,
}
```

A positional-only C function with typed extraction (`unicodedata.rs`) —
`StrArg` borrows the text zero-copy. Note the form *name* is validated in the
body (`NormForm::parse`), not during extraction: CPython type-checks every
argument before the body rejects a bad value, so value checks must not run
inside `FromValue`:

```rust
#[derive(FromArgs)]
#[from_args(name = "normalize", bad_arg)]
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
fn call_normalize(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let NormalizeArgs { form, unistr } = NormalizeArgs::from_args(args, vm)?;
    defer_drop!(unistr, vm);
    let normalized = form.apply(unistr.as_str(vm)); // sketch — do the work here
    // ... allocate and return the result; the guard releases `unistr`
}
```

### Field rules

- Declaration order = Python signature order:
    `[pos_only…] [pos_or_keyword…] [varargs] [kw_only…] [varkwargs]`, required
    fields before defaulted ones in the positional region.
- Typed fields use `FromValue` after binding. Use raw `Value` for coercion in the function body.
- Prefer `StrArg` over `String` for str params the function only reads — it
    validates without copying and lends `&str` via `.as_str(vm)`.
- Field names that aren't single ASCII chars need a `StaticStrings` variant
    (in `crate::intern`) for kwarg matching — add one, or point
    `static_string = "ExistingVariant"` at an existing one.
- `Value` / `StrArg` / `Option<Value>` fields hold heap references: bind
    them with `defer_drop!` in the body, or otherwise guarantee
    `drop_with` on every path.
- `varargs` fields must be `Vec<Value>`; convert elements in the body.

### Tests and docs

- Add behaviour tests to `crates/monty/test_cases/` (dual-run against
    CPython — asserts must pass on both engines; exact `==` messages, never
    `in`). Signature diagnostics belong in `args__macro_errors.py`.
- Document every remaining CPython divergence in `./limitations/<module>.md`
    — including "obvious" ones.
- Run `make test-cases`, `make format-rs`, `make lint-rs` (and
    `make lint-py` for test changes) before finishing.
