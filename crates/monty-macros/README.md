# monty-macros

Procedural macros used by the [`monty`](https://crates.io/crates/monty) crate.

## `#[py_signature]`

A Python signature on a native function generates the Rust argument struct and its `FromArgs` implementation.

```rust
#[py_signature("split(self, /, sep: str | None = None, maxsplit: i64 = -1) -> list")]
fn str_split<'h>(s: &PyStr<'h>, args: &SplitArgs, vm: &mut VM<'h>) -> RunResult<Value> {
    // ...
}
```

The adapter binds the arguments, then uses Monty’s existing `defer_drop!` or `defer_drop_mut!` guard for the function body.
Use `&mut` when the implementation needs to move a field out.
Each keyword parameter needs an entry with its exact Python spelling in `StaticStrings`.
With `style = def`, leave parameters unannotated and convert values in the function body.
For a Python method, `style = def, owner = "Counter"` includes the receiver in positional arity errors and qualifies the
method name.
Leave the bound `self` parameter unannotated and omit its default.
`kwarg_error_name` overrides the callable name in unexpected-keyword errors, as in `json.dump`.

For a single required positional-only argument, declare the Rust parameter directly, as in
`#[py_signature("len(obj, /)")]` with `obj: &Value`.
The macro emits the existing helper call and guards the returned value for the function body.
This preserves the helper’s arity errors, including its positional-count error for keyword calls.
A typed parameter such as `sqrt(x: float, /)` runs the converter after the same arity check.
Use a shared reference for conversions that retain a Python object, such as `number: &IntArg` or `text: &StrArg`.
The adapter releases it when the body returns.
Leave parameters unannotated when their operation runs in the body,
as with the iterable passed to `enumerate`.

For two required positional values, `#[py_signature("divmod(x, y, /)", style = two_args)]`
binds the Rust parameter `args: (&Value, &Value)`. Destructure it with `let (x, y) = args;`.
Keywords take precedence over arity errors.

For `f()` or a bound method such as `#[py_signature("keys(self, /)", owner = "dict")]`, the Rust
body can omit the argument struct. The macro adds a trailing `ArgValues` parameter and calls
`check_zero_args` before the body. An explicit parser style such as `style = def` keeps its own arity errors.

### Ownership and conversion

Take the generated struct as `args: &SplitArgs` and use its fields directly or destructure it in the body.
For example, `let SplitArgs { sep, maxsplit } = args;` borrows both fields.
When the body takes ownership, declare `args: SortedArgs` and begin with `let SortedArgs { iterable, kwargs } = args;`.
Bind every owned field by name and release it on every return path.
For mixed ownership, take `args: &mut PopArgs`. In `dict.pop`, the lookup borrows `&args.key`
and a miss moves the optional default out with `args.default.take()`. The guard releases the remaining fields.
For a required `Value`, use `mem::replace(&mut args.iterable, Value::None)` to move it out.
For callers that bind after method lookup, put `#[py_signature]` on a unit struct and call
`Name::from_args(args, vm)?`. Generated fields have the struct's visibility.

For a raw positional signature with at most two arguments, the generated struct takes its
fields from the inline argument storage. Use `reject_keywords` when keyword errors take
precedence over arity. A handler that needs a slice or iterator can take `&ArgPosIter`,
`&mut ArgPosIter`, or an owned `ArgPosIter`. The adapter checks the positional count
and preserves inline storage for small calls.

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

### `style` — pick the CPython parser family

The `style` attribute names the argument parser the target function uses in
CPython. It selects both the error *wording* and the error *ordering*
(binding errors vs conversion errors — see `ErrorFamily` in
`crates/monty/src/args/bind_native.rs` for the exact contracts). Pick it by
looking at how the function is implemented in CPython:

| CPython implementation                                                                                           | `style`                          | tell-tale error wording                                                     |
| ---------------------------------------------------------------------------------------------------------------- | -------------------------------- | --------------------------------------------------------------------------- |
| pure-Python `def` (`re` module, `json.dumps`, `Path.mkdir`)                                                      | `def`                            | `f() takes from 1 to 2 positional arguments but 3 were given`               |
| Argument Clinic — most modern builtins/methods (look for clinic blocks / `*.c.h` includes in the CPython source) | `clinic` (the default — omit it) | `replace() takes at least 2 positional arguments (1 given)`                 |
| `PyArg_ParseTupleAndKeywords`, no `:name` in the format string                                                   | `c`                              | `function missing required argument 'day' (pos 3)`                          |
| `PyArg_ParseTupleAndKeywords` with `:name`                                                                       | `c_named`                        | `timezone() missing required argument 'offset' (pos 1)`                     |
| `PyArg_UnpackTuple` (positional-only, fixed `min..max`)                                                          | `unpack`                         | `name expected at most 2 arguments, got 3` (`expected N …` when min == max) |

Style-derived behaviour that used to be separate flags: the C
"… positional arguments …" overflow pivot turns on automatically for
`style = c` and `style = c_named` structs with `kw_only` fields (CPython's
`vgetargskeywords` / `_PyArg_UnpackKeywords` behave the same way; the named
variant additionally says `takes exactly N positional argument(s)` when every
positional param is required, e.g. `os.stat`), and `unpack` collapses to the
exact-arity `expected N argument(s)` wording when no positional field has a
default.

Since CPython's `def` binding never type-checks, `style = def` structs
should declare fields as raw `Value` (or `StrArg`-in-body) and coerce in the
function body, so type errors carry the message the CPython function body
would produce. `bad_arg` is rejected under `style = def` for the same
reason. `varargs` is allowed: a `*args` signature never overflows, but a
missing required parameter still wants the `def` wording.

`style = unpack` models `PyArg_UnpackTuple`'s fixed positional `min..max`
range. Use `/` after the positional parameters. An explicit `**kwargs` captures
keywords for callables such as `sorted`, which forwards them to `list.sort`.

### Modifiers

- `at_most_total` — pre-count positionals + kwargs against all named parameters,
    including keyword-only parameters, before dispatch (`{name}() takes at most N arguments (M given)`).
    This is a per-function empirical fact, not derivable from the fields or
    the style. Litmus test: call the CPython function with valid positionals
    plus one bogus kwarg — if it reports `takes at most N arguments (M given)`, set the flag; if it reports `unexpected keyword argument`,
    don't. Only meaningful for the C-parser families (`clinic`/`c`/`c_named`)
    on signatures with a fixed maximum — rejected under `style = def` /
    `style = unpack` and with `varargs`/`varkwargs`.
- `vectorcall` — kwarg-free calls check positional arity first with
    `_PyArg_CheckPositional` wording (`{name} expected at most N arguments, got M`), modeling `tp_vectorcall` fast paths (`int`, `str`) that only
    fall back to the clinic parser when keywords are present. Requires the
    default `clinic` style plus `at_most_total` (which supplies the
    parenthesised wording for the kwargs path).
- `bad_arg` / `bad_arg_named` — report `FromValue` wrong-type failures in
    CPython's `_PyArg_BadArgument` wording (`{name}() argument {pos|'arg'} must be {expected}, not {got}`).
- `kwarg_error_name = "..."` — override the function name in the
    unknown-kwarg error only (`json.dumps` reports `JSONEncoder.__init__`).
    Under `style = unpack` it instead names the function in the blanket
    `takes no keyword arguments` rejection (`unicodedata.name()`).
- `kwargs_not_supported_yet` — reject every kwarg with a
    `NotImplementedError`; a Monty TODO marker.

Use Python's `/`, `*`, `*args` and `**kwargs` markers for parameter roles.
Keyword-only parameters currently require defaults. The generated binder exposes
`*args` as `Vec<Value>` and `**kwargs` as `KwargsValues`.

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
