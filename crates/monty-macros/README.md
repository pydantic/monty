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
implement `FromValue` (impls live in `monty::args::from_value`).

The full attribute surface — struct-level wording flags (`c_error`,
`c_error_named`, `at_most_total`, `bad_arg`, `kwargs_not_supported_yet`, …)
and field-level roles (`pos_only`, `kw_only`, `varargs`, `varkwargs`,
`default`, `static_string`) — is documented inline on `StructAttrs`, the
`FieldKind` enum, and each `render_*` helper in
[`src/from_args.rs`](src/from_args.rs).

## `#[derive(ToArgs)]`

Inverse of `FromArgs`: projects a struct into the `(Vec<MontyObject>,
kwargs)` pair host callbacks expect. Reuses the `#[from_args(...)]` field
attributes so a struct that derives both stays consistent in both
directions. Field types must implement `monty::args::ToMontyObject`.

## Not a standalone crate

Generated code emits `crate::...` paths and only compiles inside `monty`.
Cross-crate use would need `proc-macro-crate` plus switching to
`::monty::...` paths.
