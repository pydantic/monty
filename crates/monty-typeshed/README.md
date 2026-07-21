# monty-typeshed

The vendored [typeshed](https://github.com/python/typeshed) subset that powers
type checking in [Monty](https://github.com/pydantic/monty).

Monty implements a deliberately small subset of Python's stdlib. Its type
checker (the [`monty-type-checking`](https://crates.io/crates/monty-type-checking)
crate, powered by [ty](https://docs.astral.sh/ty/)) therefore needs stubs that
describe *Monty's* runtime surface rather than CPython's: code using an
unsupported builtin or stdlib module should fail type checking up front, not
pass the checker and then fail at runtime. This crate vendors only the stubs
for what Monty supports, with unsupported functions and classes filtered out.

Originally derived from ruff's
[`ty_vendored`](https://github.com/astral-sh/ruff/tree/main/crates/ty_vendored)
crate.

## Layout

- `vendor/typeshed/` — the vendored stubs, derived from upstream typeshed at
  the commit recorded in `vendor/typeshed/source_commit.txt` (exposed as
  `SOURCE_COMMIT`). **Do not edit these files manually** — they are
  overwritten wholesale by the update script.
- `custom/` — hand-written stubs that override or supplement upstream files
  where Monty's surface intentionally differs (e.g. `asyncio`, `os`, `sys`,
  `unicodedata`). The update script copies them into `vendor/typeshed/stdlib`.
- `update.py` — clones upstream typeshed, filters the stubs down to the
  builtins and modules Monty supports (whitelists in the script mirror
  `crates/monty/src/builtins/`), and applies the `custom/` overrides. Run it
  via `make update-typeshed` from the workspace root.
- `build.rs` — zips the vendored stubs into the compiled binary at build time,
  so Monty ships type checking with no files on disk.

## API

The crate exposes a single entry point: `file_system()` returns a static
`ruff_db` `VendoredFileSystem` over the zipped stubs, which
`monty-type-checking` uses as the search path for stdlib module resolution.

```rust
let typeshed = monty_typeshed::file_system();
assert!(typeshed.exists("stdlib/builtins.pyi"));
```

## Monty crates

- [`monty`](https://crates.io/crates/monty) — the core interpreter: Python parser, bytecode VM, and sandbox.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between pool parents and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements. **this crate**
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
