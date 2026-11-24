# monty-macros

Monty uses `FromArgs` inside the interpreter to bind a Python call to a Rust struct.
In the other direction, `monty-types` uses `ToArgs` to turn a Rust struct into arguments for a host callback.
Both derives generate paths relative to their owning crate.
The third macro, `explicit_discriminants`, checks the numeric codes assigned to enum variants.

## `#[derive(FromArgs)]`

Write the Python signature as a struct with named fields.
For example, `math.prod(iterable, /, *, start=1)` takes its iterable by position and its starting value by keyword.
Its [Rust implementation](../monty/src/modules/math/aggregate.rs) declares both parameters as follows.

```rust
#[derive(FromArgs)]
#[from_args(name = "math.prod")]
struct ProdArgs {
    #[from_args(pos_only)]
    iterable: Value,
    #[from_args(kw_only, default = Value::Int(1))]
    start: Value,
}
```

The handler calls `ProdArgs::from_args(args, vm)?` and owns the values in the returned struct.
The `name` attribute supplies the callable name in error messages.
When several handlers share one signature, omit `name` and pass the callable name to the derived method.
For example, the [CSV reader](../monty/src/modules/csv.rs) calls `FmtParamArgs::from_args("reader", args, vm)?`.
For a fixed pair of positional arguments, use `let (index, item) = args.get_two_args("list.insert", vm.heap)?;`.

### Parameter declarations

Declare fields in Python signature order, with required positional parameters before positional parameters with defaults.

| Field attribute | Python parameter      | Rust behavior                                                         |
| --------------- | --------------------- | --------------------------------------------------------------------- |
| `pos_only`      | Before `/`            | Binds by position.                                                    |
| No attribute    | Positional or keyword | Binds by position or by the field's name.                             |
| `varargs`       | `*args`               | Collects remaining positional arguments into `Vec<Value>`.            |
| `kw_only`       | After `*`             | Binds by keyword. Fields after `varargs` are implicitly keyword-only. |
| `varkwargs`     | `**kwargs`            | Collects unmatched keywords into `KwargsValues`.                      |

On a named parameter, `#[from_args(default)]` uses the Rust type's `Default::default()` when the argument is omitted.
Use `default = expr` for a Python default such as `Value::Int(1)`.
Keyword matching uses an interned name. Set `static_string = "IterableArg"`, for example, when the `StaticStrings`
variant for a field named `iterable` is `IterableArg`.
The derive rejects defaults and `static_string` on collectors.

### Binding errors

These calls show the messages Monty produces for the signature above and for `enumerate(iterable, start=0)`.

```pycon
>>> import math
>>> math.prod([2, 3], start=4)
24
>>> math.prod([2, 3], 4)
TypeError: math.prod() takes at most 1 positional argument (2 given)
>>> math.prod(iterable=[2, 3])
TypeError: math.prod() got some positional-only arguments passed as keyword arguments: 'iterable'
>>> enumerate(start=1)
TypeError: enumerate() missing 1 required positional argument: 'iterable'
>>> enumerate([], iterable=[])
TypeError: enumerate() got multiple values for argument 'iterable'
```

The [native binder](../monty/src/args/bind_native.rs) checks keywords before reporting excess or missing positional
arguments.
Once the arguments fit the signature, the derive converts supplied fields in declaration order using `FromValue`.
It generates direct paths for common calls with zero, one, or two positional arguments.
Both paths release owned values if conversion fails.
See the [binding error fixtures](../monty/test_cases/args__macro_errors.py) for exact Monty and CPython messages.

Use `StrArg` for a string the handler reads through `as_str(vm)`, or `Value` when the handler performs its own coercion.
The struct options `bad_arg` and `bad_arg_named` select wrong-type messages that identify the argument by position or
name for converters that declare an expected type.
Errors raised during conversion, such as `OverflowError`, propagate to the caller.
Use `kwargs_not_supported_yet` when CPython accepts keywords that the Monty handler has yet to implement.
Supplying any keyword then raises `NotImplementedError`.

## `#[derive(ToArgs)]`

`ToArgs::to_args(self)` consumes a struct and builds the `CallArgs` sent to the host.
For example, [`MkdirCallArgs`](../monty-types/src/os.rs) sends `path` as its sole positional argument and sends
`parents` and `exist_ok` as named keywords, including when their values are `false`.

Fields marked `pos_only` and ordinary fields before `varargs` become positional arguments in declaration order.
A `varargs` field contributes one positional argument per element.
Ordinary fields after `varargs` and fields marked `kw_only` become keyword arguments using their Rust field names.
A unit struct produces an empty call.
The derive rejects `varkwargs` fields.

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

Use this attribute on an enum whose numeric codes must stay stable, such as
[`ExcType`](../monty-types/src/exceptions.rs).
Assign each variant a distinct integer literal and place the attribute before any derives so the check includes
conditionally compiled variants.
Keep each existing variant's code fixed. Use an unused code for each new variant and reserve removed codes.

Start with `#[monty_macros::explicit_discriminants]` and compile to get the attribute with its checksum.
Review the mapping diff before copying a replacement checksum from the diagnostic.
The checksum is FNV-1a over sorted, NUL-terminated `name:code` pairs with codes written in decimal.
Reordering variants or changing their payloads preserves the checksum.

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
