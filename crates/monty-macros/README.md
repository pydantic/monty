# monty-macros

Procedural macros used by the [`monty`](https://crates.io/crates/monty) crate.

## `#[py_signature]`

A Python signature on a native function generates the Rust argument struct and its `FromArgs` implementation.

```rust
#[py_signature("split(self, /, sep: str | None = None, maxsplit: i64 = -1) -> list")]
fn str_split<'h>(s: &PyStr<'h>, params: &SplitArgs, vm: &mut VM<'h>) -> RunResult<Value> {
    // ...
}
```

The adapter binds the arguments and keeps them in a cleanup guard for shared or mutable access.
Name the generated struct parameter `params`. Read `*args` and `**kwargs` through `params.args` and `params.kwargs`.
Each keyword parameter needs an entry with its exact Python spelling in `StaticStrings`.
Declare the receiver as an unannotated positional-only `self` parameter without a default.
Use `owner = "Counter"` to qualify a bound method's name in binding errors.
`kwarg_error_name` overrides the callable name in unexpected-keyword errors, as in `json.dump`.

For a single argument, use the same handler shape as for several arguments.
For `#[py_signature("abs(x, /)")]`, declare `params: &AbsArgs` and read the argument through `params.x`.
The macro infers inline binding for small positional calls from the signature.
Leave parameters unannotated when their operation runs in the body, as with the iterable passed to `enumerate`.

For `f()` or a bound method such as `#[py_signature("keys(self, /)", owner = "dict")]`, the Rust
body can omit the argument struct. The macro adds a trailing `ArgValues` parameter and calls
`check_zero_args` before the body.

### Ownership and conversion

Shared borrowing is the default. Take the generated struct as `params: &SplitArgs` and access fields as `params.sep`
and `params.maxsplit`. You can also spell out `bind = shared`.
Borrow inputs that the handler reads, converts or uses to create an iterator.
To transfer bound values into a result, runtime state or a consuming API, use `bind = owned`.
Declare `params: SortedArgs` and begin with `let SortedArgs { iterable, kwargs } = params;`.
Bind every owned field by name and release it on every return path.
For mixed ownership, set `bind = mut` and take `params: &mut PopArgs`. In `dict.pop`, the lookup borrows `&params.key`
and a miss moves the optional default out with `params.default.take()`. The guard releases the remaining fields.
For a required `Value`, use `mem::replace(&mut params.iterable, Value::None)` to move it out.
For callers that bind after method lookup, put `#[py_signature]` on a unit struct and call
`Name::from_args(args, vm)?`. Generated fields have the struct's visibility.

Use `reject_keywords` when keyword errors take precedence over arity on a small raw positional signature.
When `*args` is the only parameter apart from `self`, the binder stores it in an `ArgPosIter` if there are no diagnostic modifiers.
It preserves inline storage for small calls and exposes a slice through `as_slice()`.
Signatures with other parameters collect `*args` into a `Vec<Value>`.
With `bind = owned`, consume the iterator or release the remaining values with `drop_with` before returning.
Use `bind = mut` when the handler can stop early and the guard should release the remaining values.

An unannotated parameter carries a Python value. A default of `...` distinguishes omission
from a supplied value, including `None`.
`int` binds an `IntArg`, keeping small integers inline and retaining the original storage for large integers.
`SupportsIndex` also accepts objects with `__index__`, preserving the result's full width.
Use `c_int` for a C `int` argument and `i64` for a signed 64-bit index.
`float` extracts an `f64` through `__float__`, falling back to `__index__`.
`bool` truth-tests the argument. Use `str` for borrowed text or `str | None` for nullable text.
Use `convert(real = Part)` to select a Rust type implementing `FromValue`.
When a custom converter accompanies an annotation, the converter checks the annotated input types.
`error_name` overrides the name reported in binding errors. In a `macro_rules!` template,
`#[py_signature("(iterable, r)", error_name = $name)]` uses the supplied Python identifier as the function name.

### Binding errors

Generated adapters evaluate calls in three steps.

1. Bind the call shape, including keywords, duplicates, positional overflow and missing arguments.
2. Convert supplied arguments in parameter declaration order and apply defaults.
3. Call the Rust handler for checks on the converted values.

The default binder reports keyword conflicts and unknown keywords before positional overflow and missing arguments.
Positional overflow reports the maximum allowed count and the count supplied.
Once binding succeeds, the adapter stops at the first failed conversion. Protocol methods such as `__index__`
can run at this step.

See [native call diagnostics](../../docs/limitations/builtins.md#native-call-diagnostics) for the compatibility policy.

- `bad_arg` and `bad_arg_named` add the argument position or name when a conversion reports a wrong type.
- `reject_keywords` checks for keywords before arity on a small raw positional signature.
- `kwargs_not_supported_yet` raises `NotImplementedError` when pending keyword support prevents binding.

Use Python's `/`, `*`, `*args` and `**kwargs` markers for parameter roles.
Keyword-only parameters currently require defaults. The generated binder exposes `**kwargs` as `KwargsValues`.

## `#[derive(ToArgs)]`

Projects a struct into the `CallArgs` a host callback takes, reusing the `#[from_args(...)]` field attributes.
`varargs` fields contribute one positional argument per element.

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
