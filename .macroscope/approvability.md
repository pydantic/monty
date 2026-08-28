---
conclusion: neutral
---

# Macroscope approvability (auto-approval eligibility)

These rules decide when Macroscope may auto-approve a PR versus when it must step
back and let a human review it. See https://docs.macroscope.com/approvability.

**The bar for withholding auto-approval is deliberately very high.** The default is
to auto-approve. Only genuinely high-stakes, non-correctness risk should send a PR
to a human.

## Correctness is out of scope here

Correctness (bugs, CPython divergence, sandbox and resource-limit escapes, panics,
missing `limitations/` entries, missing tests, style, naming, comment verbosity, AI
slop) is owned by the Macroscope **Correctness** check, tuned by the instructions in
`.macroscope/correctness/`. Auto-approval already waits for that check and will not
fire if Correctness fails.

So these eligibility rules must **not** withhold auto-approval for a correctness
reason. Do not hold a PR because "there might be a bug", "this looks complex", "the
sandbox could escape", or "the tests could be stronger". If the Correctness check
passes, treat the code as correct and do not re-litigate it here.
This file evaluates one thing only: does this change carry non-correctness risk large
enough that a human must sign off even when the code is correct?

## Withhold auto-approval only for these categories

Require human review only when the PR **clearly and materially** does one of the
following. A drive-by, comment-only, or cosmetic touch to one of these areas is not
enough: the change must materially alter behaviour in the category.

1. **`unsafe` and the core safety boundary.** Any change to `unsafe` code or to the
   invariants it rests on, especially `crates/monty/src/heap.rs` (pointer arithmetic,
   `UnsafeCell` access, reader-count invariants). A soundness mistake here is
   memory-unsafe, not merely wrong.

2. **The sandbox boundary.** Changes to how untrusted code is confined:
   `crates/monty-fs/` (path security, mounts, symlink resolution), the syscall/OS
   surface, or anything that could let sandboxed Python reach the host filesystem,
   environment, network, or subprocesses.

3. **The wire protocol and host/child trust boundary.** Changes to `monty-proto`
   frame decoding or proto->Rust conversion, or to `monty-pool`'s handling of frames
   from a child. These run in host/parent context where a mistake takes down the
   embedder, and they parse hostile input.

4. **Snapshot / dump format.** Changes to the serialization or on-disk/on-wire format
   of snapshots or session dumps. These are a compatibility and trust contract; a
   format change can silently break or mis-validate signed state.

5. **Public API / ABI of the crates and bindings.** Removing or renaming an exported
   item, or changing the type or shape of something `pydantic_monty`
   (`crates/monty-python`), `@pydantic/monty` (`crates/monty-js`), or a Rust embedder
   of the `monty` crate depends on. (Adding new, not-yet-depended-on API does not
   count.)

6. **Release and supply chain.** Changes to how crates/npm/PyPI artifacts are built,
   versioned, or published; workflow changes that handle publish credentials; or new
   runtime dependencies from a source we do not already trust. (Routine dev-only CI
   tweaks and lockfile bumps of existing dependencies do not count.)

## Not reasons to withhold auto-approval

None of the following, on their own, require a human. Auto-approve them when
Correctness passes:

- Large diffs, many files, or a broadly-scoped but mechanical change.
- Refactors, renames, and moves that keep behaviour the same.
- New language features, builtins, or method implementations (new behaviour, not a
  contract change) -- even large ones.
- New CPython-divergence `limitations/` entries and documentation.
- Bug fixes that do not touch the categories above.
- Test-only changes, playground probes, comments, and configuration.
- Dependency version bumps for dependencies we already use.
- A general feeling that the change is important or that review would be "safer".

## Decision rule

When in doubt, auto-approve. Withhold auto-approval only when the PR unambiguously
and materially lands in one of the categories above. The question is never "could a
human add value here?" (a human always could); it is "is the non-correctness risk of
this change high enough that it must not merge without a human deciding?".
