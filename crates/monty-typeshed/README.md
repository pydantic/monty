# monty-typeshed

The vendored [typeshed](https://github.com/python/typeshed) subset that powers
type checking in [Monty](https://github.com/pydantic/monty).

Monty implements a deliberately small subset of Python's stdlib. Its type
checker (the [`monty-type-checking`](https://crates.io/crates/monty-type-checking)
crate, powered by [ty](https://docs.astral.sh/ty/)) therefore needs stubs that
describe Monty's runtime surface rather than CPython's, so the checker can
flag unsupported functions and classes before execution. The stubs also
include some dependencies used only for checking. Imports from omitted
dependencies can resolve to `Unknown`.

Originally derived from ruff's
[`ty_vendored`](https://github.com/astral-sh/ruff/tree/main/crates/ty_vendored)
crate.

Monty's code is MIT-licensed.
`vendor/typeshed/LICENSE` contains the upstream Apache-2.0 licence and the MIT notice for parts of typeshed.
Modified upstream stubs identify the changes in their source.

## Layout

- `vendor.toml` pins the upstream commit, lists the selected stubs and declares any files added by patches.
- `patches/` contains the ordered changes to the upstream files, including builtin pruning and the module list in `stdlib/VERSIONS`.
- `vendor/typeshed/` is the generated tree. Its `source_commit.txt` is exposed as `SOURCE_COMMIT`.
- `build.rs` zips the vendored stubs into the compiled binary at build time, so Monty ships type checking with no files on disk.

## Updating stubs

Run `make update-typeshed` from the workspace root to download the pinned files, apply the patches in file-name order and refresh the type-checker warm-up snippet and worker licence bundle.
`make check-typeshed` downloads the same pinned files and compares the result with the checked-in tree without rewriting it.

To add a patch, edit the stubs under `vendor/typeshed/stdlib/` and save the edits with:

```sh
uv run scripts/vendor_typeshed.py --make-patch NNNN-name --reason 'Why Monty needs this change'
make update-typeshed
```

The name must sort last; using the last patch's name replaces it.
To change an earlier patch, edit it directly and check that the patches after it still apply.
List new upstream files in `vendor.toml` and declare Monty-only files in its `additions` list before saving their patch.
Update the `VERSIONS` patch for modules the type checker should resolve.

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
- [`monty-types`](https://crates.io/crates/monty-types) — the shared boundary data types (values, exceptions, OS calls, resource limits) hosts use without linking the interpreter.
- [`monty-fs`](https://crates.io/crates/monty-fs) — host-side filesystem mounts: maps virtual sandbox paths to real host directories.
- [`monty-runtime`](https://crates.io/crates/monty-runtime) — the `monty` binary: REPL, file runner, and subprocess worker mode.
- [`monty-pool`](https://crates.io/crates/monty-pool) — an elastic pool of crash-isolated `monty` worker subprocesses.
- [`monty-proto`](https://crates.io/crates/monty-proto) — the protobuf wire protocol spoken between clients and workers.
- [`monty-type-checking`](https://crates.io/crates/monty-type-checking) — type checking of sandboxed code, powered by [ty](https://docs.astral.sh/ty/).
- [`monty-typeshed`](https://crates.io/crates/monty-typeshed) — the trimmed typeshed stubs describing the stdlib subset Monty implements. **this crate**
- [`monty-macros`](https://crates.io/crates/monty-macros) — the proc macros behind `monty`'s argument parsing.
