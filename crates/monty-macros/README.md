# monty-macros

Procedural macros used by the [`monty`](../monty/) crate. Not a public crate
— consumers of `monty` get the macros re-exported (e.g. `monty::args::FromArgs`)
and should not depend on `monty-macros` directly.

## What's here

- `#[derive(FromArgs)]` — re-exported as `monty::args::FromArgs`. Generates
  argument-extraction code for Rust-implemented Python functions, methods,
  type constructors, and `OsFunction`s.

## `#[derive(FromArgs)]`

Builtins, type constructors, and OS-call handlers in monty receive a
[`ArgValues`](../monty/src/args/mod.rs) — a tagged enum of (up to two
inline) positional values plus optional kwargs. Hand-written extractors
have to:

1. Split into positional iterator + kwargs.
2. For each positional: dispatch by index, coerce to the right Rust type,
   and `defer_drop!` the input `Value`.
3. For each kwarg: read the key as a string, match against expected
   keyword names, reject duplicates and unknowns, coerce the value.
4. Validate required fields and apply defaults.
5. **On every error path**, drop already-extracted owning fields so
   reference counts stay balanced.

`#[derive(FromArgs)]` generates all of the above from a struct declaration
that reads like a Python signature.

### Quick example

```rust
use monty::args::{ArgValues, FromArgs};
use monty::value::Value;

#[derive(FromArgs)]
#[from_args(name = "function", c_error, at_most_positional)]
struct DatetimeInitArgs {
    year: i32,
    month: i32,
    day: i32,
    #[from_args(default = 0)]
    hour: i32,
    #[from_args(default = 0)]
    minute: i32,
    #[from_args(default = 0)]
    second: i32,
    #[from_args(default = 0)]
    microsecond: i32,
    #[from_args(default = Value::None)]
    tzinfo: Value,
}

// In your function:
let DatetimeInitArgs { year, month, day, hour, minute, second, microsecond, tzinfo } =
    DatetimeInitArgs::from_args(args, heap, interns)?;
```

(`c_error` here opts into CPython's `PyArg_ParseTupleAndKeywords` wording —
"function got an unexpected keyword argument 'X'" etc. — which is what
C-implemented constructors like `datetime` use. Default is Python-method
wording, used by most builtin methods and pure-Python functions.)

The generated `from_args` method returns `RunResult<Self>` and is guaranteed
to drop every `Value` it touches on every error path.

### Field types

A field's type must implement the `monty::args::FromValue` trait. Built-in
impls live in [`crates/monty/src/args/from_value.rs`](../monty/src/args/from_value.rs)
and cover `Value`, `i32`, `i64`, `bool`, `String`, and `Option<T>`. Add a
new impl there if you need to extract a type that isn't covered.

### Attribute surface

#### Struct-level (`#[from_args(...)]` on the struct itself)

- `name = "..."` (required) — function name embedded in error messages.
  Used as the `{name}()` prefix in Python-method-style errors and (with
  `c_error`) as the descriptor in the positional/keyword conflict
  message. C-implemented constructors typically pass `"function"` here
  to match CPython's wording.
- `c_error` — use C-constructor error wording (matches CPython's
  `PyArg_ParseTupleAndKeywords` — e.g. `datetime`):
  - unknown kwarg: `this function got an unexpected keyword argument 'X'`
  - pos/kw conflict: `argument for function given by name ('Y') and position (N)`
  - missing required: `function missing required argument 'Y' (pos N)`
  - too many positional: `function takes at most M arguments (N given)`

  Default (no `c_error`) is Python-method wording, matching `def`-defined
  functions and most builtin methods (`list.sort`, `datetime.replace`):
  - unknown kwarg: `{name}() got an unexpected keyword argument 'X'`
  - pos/kw conflict: `{name}() got multiple values for keyword argument 'Y'`
  - missing required: `{name}() missing 1 required positional argument: 'Y'`
  - too many positional: `{name} expected at most M arguments, got N`
- `c_error_named` — mutually exclusive with `c_error`. Use this for
  C-implemented constructors that *do* embed the function name in their
  error messages (matches CPython's `timezone`, the `re` module functions,
  etc.):
  - unknown kwarg: `{name}() got an unexpected keyword argument 'X'`
    (same wording as the default Python style)
  - pos/kw conflict: `argument for {name}() given by name ('Y') and position (N)`
    (C wording with the name as the descriptor)
  - missing required: `{name}() missing required argument 'Y' (pos N)`
  - too many positional: `{name}() takes at most M arguments (N given)`
- `at_most_positional` — only meaningful with `c_error`. Switches the
  too-many-args error to `"function takes at most M positional arguments
  (N given)"` (matches `datetime`). Default is the plain `"function takes
  at most M arguments (N given)"` wording (matches `date`). The
  `c_error_named` and default Python styles ignore this flag.
- `at_most_total` — pre-counts `positional + kwarg` and raises
  `"… takes at most M argument(s) (N given)"` *before* per-arg dispatch.
  Matches CPython's `PyArg_ParseTupleAndKeywords` semantics so e.g.
  `'a'.expandtabs(8, tabsize=4)` reports
  `"str.expandtabs() takes at most 1 argument (2 given)"` rather than
  the per-arg pos/kw conflict wording. Set this on any struct that
  models a CPython C function/method exposing this behaviour (`date`,
  `timezone`, `expandtabs`, `splitlines`, `groupdict`, …). The wording
  comes from `type_error_c_at_most[_positional]` (with `c_error`) or
  `type_error_method_at_most` (with default / `c_error_named`).
  Mutually exclusive with `varargs` / `varkwargs` — the pre-count is
  meaningless for signatures with an unbounded maximum.

#### Field-level (`#[from_args(...)]` on a field)

- `default` — use `Default::default()` when the argument is absent.
- `default = <expr>` — use `<expr>` when the argument is absent.
- `pos_only` — accept positionally only; rejects the field as a keyword
  argument with "got some positional-only arguments passed as keyword
  arguments".
- `kw_only` — accept as keyword only; rejects the field as a positional
  argument.
- `varargs` — field type must be `Vec<T>`; collects all remaining
  positionals. Only one per struct. Implicit `*` separator for kw_only
  fields.
- `varkwargs` — field type must be `KwargsValues`; collects unmatched
  kwargs. Only one per struct.
- `static_string = "Variant"` — override the auto-derived
  `StaticStrings::PascalCase(field_ident)` used for kwarg dispatch.

### Field ordering

Fields must appear in Python-signature order — the macro enforces this at
compile time with `compile_error!` messages:

```text
[pos_only...] [pos_or_keyword...] [varargs] [kw_only...] [varkwargs]
```

Within each region required fields must precede optional ones.

### Kwarg dispatch via `EitherStr::matches`

The generated code dispatches each kwarg by calling
`EitherStr::matches(StaticStrings::PascalCase(field_ident).into(), interns)`.
That helper accepts both an interned `StringId` (the fast path —
`__id == target`) and a heap-allocated `Heap(String)` (which falls back
to a byte-for-byte string comparison against the interned spelling). The
latter is what makes `f(**{some_dynamic_name: ...})` work even though the
dispatch dictionary is built around `StaticStrings`.

If a field name has no matching `StaticStrings` variant, the *user-side*
`rustc` build fails with `no variant <X> on enum StaticStrings`. Add the
variant to [`crates/monty/src/intern.rs`](../monty/src/intern.rs) — this
is the intended workflow and keeps the fast path as cheap as the
hand-written code.

If your field name doesn't fit the auto-derived pascalisation (or
collides with an existing variant used for an attribute name — for
example `string` is taken by `match.string`), use
`#[from_args(static_string = "Variant")]` to point at a different
`StaticStrings` variant. Pos-only fields can also use
`#[from_args(static_string = "...")]` to *opt into* the
"positional-only-argument-passed-as-keyword" rejection arm; without it
the macro skips that arm and a kwarg matching the field name falls
through to the generic "unexpected keyword" error.

### Refcount safety

Two mechanisms keep refcounts balanced in generated code:

- `FromValue::from_value` consumes the input `Value` and is responsible
  for dropping it on both success and failure. The `Value` impl is the
  one exception — it transfers ownership of the value into the struct.
- The generated `from_args` body holds extracted owning fields in
  `Option<T>` slots and emits a local `__cleanup!` macro that drops every
  slot before bubbling an error. Every error path goes through
  `__cleanup!`, including the two iterators that haven't been consumed yet.

This pattern matches how hand-written extractors use `defer_drop!` /
`HeapGuard`, just generated mechanically.

## Limitations and out-of-scope

- The generated code emits `crate::...` paths and only works from inside
  the `monty` crate. Cross-crate usage would need `proc-macro-crate` plus
  switching to `::monty::...` paths.
- A handful of CPython constructors use bespoke error wording (e.g.
  `date()` reports kwarg-overflow distinctly from positional-overflow).
  These need new attributes or new helpers in
  [`exception_private.rs`](../monty/src/exception_private.rs) before they
  can be migrated.

## When to migrate a callsite

Strong candidates:

- Constructors and methods with **multiple positional or keyword
  arguments**, especially with defaults and positional/keyword conflict
  detection. The `datetime` constructor went from 151 lines to 25.

Weak candidates / don't bother:

- 0-, 1-, or 2-argument helpers that the existing `ArgValues::check_zero_args` /
  `get_one_arg` / `get_two_args` / `get_one_two_args` cover in a single
  line. The macro isn't shorter than those.
- Functions that need bespoke kwarg validation logic (e.g. `open()`'s
  "accept but discard" kwargs), unless an attribute is added to support
  that pattern.

## Adding a new feature

Generation lives in [`src/from_args.rs`](src/from_args.rs). The
high-level pipeline:

1. `Signature::parse` parses the struct + attrs into a validated AST.
2. `Signature::render` emits the `impl Self { fn from_args(...) }` block
   via four sub-renderers: slot declarations, the `__cleanup!` macro
   body, the positional dispatch loop, and the kwarg dispatch loop, then
   the final `Self { ... }` constructor.

When adding new attributes:

1. Extend `parse_struct_attrs` or `parse_field_attrs` to recognise the
   new syntax.
2. Plumb the resulting state through `Signature` / `Field`.
3. Adjust the relevant `render_*` method to emit the new code.
4. Update this README's attribute table.

When adding support for a new `FromValue` type, put the impl in
[`crates/monty/src/args/from_value.rs`](../monty/src/args/from_value.rs) —
the macro doesn't need to know about new field types, just dispatch via
`<#ty as FromValue>::from_value(...)`.
