# CPython stdlib tests

`make test-cpython` type-checks the suites enabled in [`vendor.toml`](../../../monty/stdlib/vendor.toml) and runs them on Monty and CPython.
It fetches the pinned CPython archive into `work/cache/` on the first run, then extracts `Lib/test`, the license, and the listed support files into `work/generated/` and applies the test patches.
Both directories are gitignored.
`make test-cases` does not need these files.

To run a single suite, run `make prepare-cpython-tests` first, then use:

```sh
cargo run -p monty-datatest --features cpython-tests -- test_keyword.py --nocapture
```

## Discovery and exclusions

CPython runs the full suite in a subprocess with its own `unittest`, resolving `test.support` and fixtures from the prepared tree.
The Monty runner uses embedded CPython's `unittest` to discover methods, then imports the suite and the patched [`unittest` package](patches/0003-unittest-package.patch) from a read-only mount of `work/generated/Lib`.
The package is not bundled with the runtime.
It creates a new instance per method, runs `setUp` and `tearDown`, and drains the cleanup stack in LIFO order even after an error.
The Monty runner stops on the first failing method or subtest.
It does not implement class or module fixtures, runtime skips, or expected failures.

[`skips.toml`](skips.toml) affects Monty only.
Under `[suites."<file>".exclude]`, list exact `unittest` IDs with reasons.
If the file cannot load and has no applicable methods, use `skip = "<reason>"` under `[suites."<file>"]` instead.
For a broad suite that cannot load but has applicable methods, list their `ClassName.test_name` under `[tests.project]`
in `vendor.toml`.
`make prepare-cpython-tests` extracts those methods into stand-alone `unittest.TestCase` classes in `work/generated/monty/`.
The generated modules import `sys` and `unittest`; put any other imports the methods need inside the methods and list
any fixture files they import in `[tests].support`.
A top-level `if sys.platform != 'monty':` block in a selected method is omitted before Monty parses it.
Use it for unsupported cases inside a method without removing their coverage on CPython, which runs the full suite.
Stale IDs and enabled suites with no runnable methods are errors.
Use `--nocapture` to see manifest exclusions and native `unittest` runtime skips.

## Type checking

Each suite's type check runs separately from its runtime tests and fails on any diagnostic, including unused ignores.
It checks the source staged for Monty, including methods excluded from runtime, using Monty's normal stdlib stubs and a test-only [`unittest.pyi`](unittest.pyi).
The checker does not resolve imports from the runtime mount.
Use rule-specific `# ty: ignore[...]` comments for deliberate runtime probes outside the stubs or for missing APIs in excluded methods.
Keep those comments in a separate patch, and remove them when the relevant stub changes make them unnecessary.

## Adding a suite

Add the suite path relative to `Lib/test` to `[tests].suites` in `vendor.toml`.
List any additional files outside `Lib/test`, or helper files that need patches, in `[tests].support` relative to `Lib`.
Run `make prepare-cpython-tests`, then edit the generated files and save the changes with `scripts/vendor_stdlib.py --test --make-patch … --reason '…'`.
Preparing the tree again overwrites local edits, so save the patch first.
Keep upstream imports, inputs, and assertions where possible.
The harness imports the suite as a module, so a guarded `unittest.main()` does not run.
If a missing import is only needed by excluded methods, move it into those methods so the rest of the suite can still load.
