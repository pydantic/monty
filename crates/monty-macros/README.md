# monty-macros

Procedural macros used by the [`monty`](https://crates.io/crates/monty) crate.
Not a public crate — consumers get the macros re-exported
(`monty::args::FromArgs` / `monty::args::ToArgs`) and should not depend on
`monty-macros` directly.

## `#[derive(FromArgs)]`

Generates the `from_args` body for a Rust-implemented Python function. The
macro is deliberately thin: it emits a `static ParamSpec` describing the
signature and one call to the runtime binder
(`crates/monty/src/args/bind_native.rs`), which owns all positional/kwarg
dispatch, arity checks, duplicate detection, and reference-count cleanup.
The generated code then converts each raw slot via `FromValue` in
declaration order and applies defaults.

```rust
use monty::args::FromArgs;
use monty::value::Value;

#[derive(FromArgs)]
#[from_args(name = "function", style = c)]
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

### `style` — pick the CPython parser family

The `style` attribute names the argument parser the target function uses in
CPython. It selects both the error *wording* and the error *ordering*
(binding errors vs conversion errors — see `ErrorFamily` in
`crates/monty/src/args/bind_native.rs` for the exact contracts). Pick it by
looking at how the function is implemented in CPython:

| CPython implementation | `style` | tell-tale error wording |
|---|---|---|
| pure-Python `def` (`re` module, `json.dumps`, `Path.mkdir`) | `def` | `f() takes from 1 to 2 positional arguments but 3 were given` |
| Argument Clinic — most modern builtins/methods (look for clinic blocks / `*.c.h` includes in the CPython source) | `clinic` (the default — omit it) | `replace() takes at least 2 positional arguments (1 given)` |
| `PyArg_ParseTupleAndKeywords`, no `:name` in the format string | `c` | `function missing required argument 'day' (pos 3)` |
| `PyArg_ParseTupleAndKeywords` with `:name` | `c_named` | `timezone() missing required argument 'offset' (pos 1)` |
| `PyArg_UnpackTuple` (positional-only, fixed `min..max`) | `unpack` | `name expected at most 2 arguments, got 3` (`expected N …` when min == max) |

Style-derived behaviour that used to be separate flags: the C
"… positional arguments …" overflow pivot turns on automatically for
`style = c` structs with `kw_only` fields (CPython's `vgetargskeywords`
behaves the same way), and `unpack` collapses to the exact-arity
`expected N argument(s)` wording when no positional field has a default.

Since CPython's `def` binding never type-checks, `style = def` structs
should declare fields as raw `Value` (or `StrArg`-in-body) and coerce in the
function body, so type errors carry the message the CPython function body
would produce. `bad_arg` is rejected under `style = def` for the same
reason, and so is `varargs` (a `*args` signature can never raise
too-many-positional, so the style would have no effect).

`style = unpack` models `PyArg_UnpackTuple`'s fixed positional `min..max`
range, so the derive rejects anything outside that shape: every positional
field must be `pos_only`, and `varargs`, `varkwargs`, and `at_most_total`
are all incompatible.

### Modifiers

- `at_most_total` — pre-count positionals + kwargs against the positional
  maximum before dispatch (`{name}() takes at most N arguments (M given)`).
  This is a per-function empirical fact, not derivable from the fields or
  the style. Litmus test: call the CPython function with valid positionals
  plus one bogus kwarg — if it reports `takes at most N arguments (M
  given)`, set the flag; if it reports `unexpected keyword argument`,
  don't. Only meaningful for the C-parser families (`clinic`/`c`/`c_named`)
  on signatures with a fixed maximum — rejected under `style = def` /
  `style = unpack` and with `varargs`/`varkwargs`.
- `bad_arg` / `bad_arg_named` — report `FromValue` wrong-type failures in
  CPython's `_PyArg_BadArgument` wording (`{name}() argument {pos|'arg'}
  must be {expected}, not {got}`).
- `kwarg_error_name = "..."` — override the function name in the
  unknown-kwarg error only (`json.dumps` reports `JSONEncoder.__init__`).
- `kwargs_not_supported_yet` — reject every kwarg with a
  `NotImplementedError`; a Monty TODO marker.

Field-level attributes: `pos_only`, `kw_only` (must carry a `default` — the
runtime binder's fast paths skip the missing-keyword check, so required
keyword-only params are rejected at derive time), `varargs` (must be
`Vec<Value>` — elements are handed over unconverted), `varkwargs`,
`default[ = expr]`, `static_string = "..."` (override the `StaticStrings`
variant used for kwarg matching; neither applies to `varargs`/`varkwargs`
fields). All are documented inline in
[`src/from_args.rs`](src/from_args.rs), which also carries `#[cfg(test)]`
unit tests for every attribute-validation error.

## `#[derive(ToArgs)]`

Inverse of `FromArgs`: projects a struct into the `(Vec<MontyObject>,
kwargs)` pair host callbacks expect. Reuses the `#[from_args(...)]` field
attributes so a struct that derives both stays consistent in both
directions. Field types must implement `monty::args::ToMontyObject`.

## Not a standalone crate

Generated code emits `crate::...` paths and only compiles inside `monty`.
Cross-crate use would need `proc-macro-crate` plus switching to
`::monty::...` paths.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing. **this crate**
