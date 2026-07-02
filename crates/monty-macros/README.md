# monty-macros

Procedural macros used by the [`monty`](../monty/) crate. Not a public crate
— consumers get the macros re-exported (`monty::args::FromArgs` /
`monty::args::ToArgs`) and should not depend on `monty-macros` directly.

## `#[derive(FromArgs)]`

Generates the `from_args` body for a Rust-implemented Python function. Each
field reads like a Python parameter, and the generated code handles
positional/kwarg dispatch, defaults, duplicate detection, type coercion via
`FromValue`, and reference-count cleanup on every error path.

```rust
use monty::args::FromArgs;
use monty::value::Value;

#[derive(FromArgs)]
#[from_args(name = "datetime", c_error, at_most_positional)]
struct DatetimeArgs {
    year: i32,
    month: i32,
    day: i32,
    #[from_args(default = 0)]
    hour: i32,
    #[from_args(default = Value::None)]
    tzinfo: Value,
}

let DatetimeArgs { year, month, day, hour, tzinfo } =
    DatetimeArgs::from_args(args, vm)?;
```

Fields must appear in Python signature order:
`[pos_only…] [pos_or_keyword…] [varargs] [kw_only…] [varkwargs]`, with
required fields before optional ones in each region. Field types must
implement `FromValue` (impls live in `monty::args::from_value`). Coercion
failures are structured (`FromValueFail`): wrong-type failures get their
wording from the extraction site (`bad_arg`/`bad_arg_named`, or the impl's
`type_error`), while value-level failures (`ValueError`, `OverflowError`)
surface unchanged. `str` arguments the function only reads should use
`StrArg`, which validates without copying the text and lends `&str` via
`as_str(vm)`.

`FromValue`-typed fields are only correct for functions **implemented in C
in CPython**, whose argument clinic type-checks while binding. See the
`py_def` note below for pure-Python `def`s.

The full attribute surface — struct-level wording flags (`c_error`,
`c_error_named`, `at_most_total`, `expected_exact`, `unpack_tuple`, `py_def`,
`bad_arg`, `kwargs_not_supported_yet`, …) and field-level roles (`pos_only`,
`kw_only`, `varargs`, `varkwargs`, `default`, `static_string`) — is documented
inline on `StructAttrs`, the `FieldKind` enum, and each `render_*` helper in
[`src/from_args.rs`](src/from_args.rs).

`expected_exact` and `unpack_tuple` select CPython's `PyArg_UnpackTuple` arity
wording (`expected N arguments, got M` / `expected at {least,most} N …`) for
positional-only builtins — `expected_exact` for a fixed count, `unpack_tuple`
for a `min..max` range (e.g. `unicodedata.name(chr[, default])`).

`py_def` marks a callable that is a pure-Python `def` in CPython (e.g. the
`re` module functions, `json.dumps`): too-many-positional errors use the
`def` wording — `f() takes [from {min} to] {max} positional argument(s) but
{N} were given`, counting positionals only, with CPython's `(and N
keyword-only argument(s))` suffix. Missing required names are aggregated into
one error (`missing 2 required positional arguments: 'a' and 'b'`) for every
Python-style struct, `py_def` or not. Since CPython's `def` binding never
type-checks, `py_def` structs should declare fields as raw `Value` and coerce
in the function body — a `FromValue` coercion failure at extraction time
would wrongly preempt later binding errors.

## `#[derive(ToArgs)]`

Inverse of `FromArgs`: projects a struct into the `(Vec<MontyObject>,
kwargs)` pair host callbacks expect. Reuses the `#[from_args(...)]` field
attributes so a struct that derives both stays consistent in both
directions. Field types must implement `monty::args::ToMontyObject`.

## Not a standalone crate

Generated code emits `crate::...` paths and only compiles inside `monty`.
Cross-crate use would need `proc-macro-crate` plus switching to
`::monty::...` paths.
