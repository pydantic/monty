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
#[from_args(name = "datetime")]
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
required positional fields before optional positional fields. Field types must
implement `FromValue` (impls live in `monty::args::from_value`). Coercion
failures are structured (`FromValueFail`): wrong-type failures get their
wording from the extraction site (`bad_arg`/`bad_arg_named`, or the impl's
`type_error`), while value-level failures (`ValueError`, `OverflowError`)
surface unchanged. `str` arguments the function only reads should use
`StrArg`, which validates without copying the text and lends `&str` via
`as_str(vm)`.

### Signature checks

The derive builds a constant signature from the fields.
Binding fills the positional and keyword slots, checks required arguments, then converts the supplied values.
A mismatch raises `TypeError` describing the binding error. For example, `enumerate(start=1)` reports
`enumerate() missing 1 required positional argument: 'iterable'`.

Specify the callable name with `#[from_args(name = "...")]`.
When several callables share a struct, omit the name and pass it as the first argument to `from_args`.

Handlers that take positional values directly can use the same signature checks without declaring a struct.
Pass the callable name to the extraction helper.

```rust
let value = args.get_one_arg("len", vm.heap)?;
// In the list.insert handler:
let (index, item) = args.get_two_args("list.insert", vm.heap)?;
```

### Error patterns

Positional-only callables report argument counts. Callables that accept keywords can identify individual parameters.

| Binding error | Message pattern |
| --- | --- |
| Arguments passed to a zero-argument callable | `f() takes no arguments (M given)` |
| Wrong count for a fixed positional-only signature | `f() takes exactly N arguments (M given)` |
| Too few for a positional-only signature with optional or variadic arguments | `f() takes at least N arguments (M given)` |
| Too many positional arguments | `f() takes at most N arguments (M given)` |
| Too many positional arguments with keyword-only parameters or `**kwargs` | `f() takes at most N positional arguments (M given)` |
| Keyword passed to a positional-only callable | `f() takes no keyword arguments` |
| Required parameter missing from a keyword-capable call | `f() missing N required positional arguments: 'a' and 'b'`, or `required keyword-only arguments` |
| Positional-only parameter passed by keyword to a callable that accepts other keywords | `f() got some positional-only arguments passed as keyword arguments: 'a'` |
| Unknown keyword | `f() got an unexpected keyword argument 'name'` |
| Parameter filled twice | `f() got multiple values for argument 'name'` |
| Non-string keyword key | `keywords must be strings` |
| Keyword support is unimplemented (`NotImplementedError`) | `f() does not yet support keyword arguments` |

The count messages use `argument` for a count of one.

### Conversion and keyword options

- `bad_arg` / `bad_arg_named` — report `FromValue` wrong-type failures in
  CPython's `_PyArg_BadArgument` wording (`{name}() argument {pos|'arg'}
  must be {expected}, not {got}`).
- `kwargs_not_supported_yet` — reject every kwarg with a
  `NotImplementedError`; a Monty TODO marker.

Field-level attributes: `pos_only`, `kw_only`, `varargs` (must be
`Vec<Value>` — elements are handed over unconverted), `varkwargs`,
`default[ = expr]`, `static_string = "..."` (override the `StaticStrings`
variant used for kwarg matching; neither applies to `varargs`/`varkwargs`
fields). All are documented inline in
[`src/from_args.rs`](src/from_args.rs), which also carries `#[cfg(test)]`
unit tests for every attribute-validation error.

## `#[derive(ToArgs)]`

Projects a struct into the `CallArgs` a host callback takes, reusing the `#[from_args(...)]` field attributes.
`varargs` fields contribute one positional argument per element.

## `#[pyattrs]`

Collects Python-visible functions on a namespace implementing `PyAttrs` inside `monty`.
The macro keeps their native bodies in an inherent implementation and generates `PyAttrs::TABLE`.
The table contains enumerable entries and a lookup function generated from the same declarations.
`TypeDef::attrs` registers each native namespace once.
Lookup follows the native MRO and retains the defining namespace for descriptor receiver checks.
Class methods receive the accessing class as a borrowed `TypeRef`; the caller keeps that class alive during the call.
Bases with different native payloads register a `TypeDef::subtype_call` adapter for their instance methods.

Each annotation names its `StaticStrings` key explicitly:

- `#[pymethod(Sort)]` registers an instance method with a native receiver, `ArgValues`, and `&mut VM`.
  Methods return `RunResult<Value>` or `RunResult<CallResult>`; the adapter converts values and preserves suspension.
  Inherited methods are registered only on their defining base; MRO lookup supplies the owner for receiver validation.
- `#[pyclassmethod(Fromkeys)]` registers `(StaticStrings, TypeRef, ArgValues, &mut VM) -> RunResult<CallResult>`.
  The adapter supplies the registered key and bound class, including calls through instances.
- `#[pyvalue(Min)]` registers `(&mut VM) -> Value`.

`Receiver` declares the view borrowed from `PyValue`:
`HeapRead<'heap, T>`, `HeapReadMut<'heap, T>`, `HeapObjectRead<'heap, T>`,
`PyStr<'heap>`, `PyBytes<'heap>`, `PyInt<'heap>`, `f64`, or `bool`.
Heap methods can take `&HeapRead`, `&HeapReadMut`, or `&HeapObjectRead`; existing handlers may also take
`&mut HeapObjectRead` when they need the guard itself mutably.
String, bytes, and integer handlers take shared references to their logical views.
In particular, integer handlers use `PyInt` to cover both small and heap integers; an `i64` receiver is unsupported.
Immediate float and bool receivers are passed by value.
An incompatible native view raises `TypeError` and releases the arguments.

Rust resolves the trait path, including aliases, and checks handler signatures and receiver type arguments.
The macro selects the receiver projection and checks duplicate attribute keys.
Conditional registrations are rejected because a table entry and its handler must have the same availability.
Functions retain their visibility, lifetime parameters, documentation, and bodies.
Unannotated helper functions are preserved without registration.

## `#[explicit_discriminants(checksum = ...)]`

Requires an explicit integer literal on every enum variant used as a numeric identity code,
and a matching `checksum` argument.
The checksum is FNV-1a over sorted, NUL-terminated `name:code` pairs, with codes normalized to decimal.
Declaration order, integer-literal spelling and variant payloads do not affect it.
Existing codes must never change or be reused, including after a variant is removed.
New variants take unused codes.

To initialize the checksum, compile with `#[monty_macros::explicit_discriminants]`: the error reports the current checksum as a copyable attribute.
A mismatch reports both values and the replacement attribute; review the mappings before updating it.
Place the attribute before any derives so it checks every declared variant, including inactive `cfg` and `cfg_attr` variants.
One checksum covers every configuration.
The macro returns the enum unchanged, preserving its attributes and representation.
The macro rejects duplicate numeric codes even between mutually exclusive variants.

## Not a standalone crate

Argument-conversion code emits `crate::...` paths: `FromArgs` compiles inside `monty`, and `ToArgs` inside `monty-types`.
Cross-crate use would need `proc-macro-crate` and paths to the appropriate crate.

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between clients and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements.
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing and identity-code checks. **this crate**
