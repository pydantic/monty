# Adding / extending stdlib modules

Match CPython's accepted calls, results and exception types for supported operations.
Native call diagnostics may differ in wording or in which error is reported first for an invalid call.

## Declare parameters with `#[py_signature]`

Declare the Python parameter list on the Rust function.
When a shared dispatcher binds inside a selected arm, use a named signature struct and `Args::from_args(args, vm)?`.
The `ArgValues` helpers are for trivial fixed-arity positional calls.
The [macro reference](../../../monty-macros/README.md) covers defaults, converters and ownership.

Annotate arguments the binder converts. Leave arguments unannotated when the operation itself must convert them.
Use `=...` when omission differs from an explicit `None`.
The adapter releases borrowed arguments on return. Handlers that take ownership must consume or guard each heap reference.

## Tests and docs

- Add behaviour tests to `crates/monty/test_cases/`, which runs against Monty and CPython.
  For native binding diagnostics that differ, check the exception type and any relevant common context, such as the offending parameter.
- Document differences in supported calls or operation behaviour in the relevant `limitations/` page.
- Run `make test-cases`, `make format-rs`, `make lint-rs` (and `make lint-py` for test changes) before finishing.
