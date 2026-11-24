# Native continuations: progress

Implements step 1 of the plan in pydantic/monty#1374 ("serializable native continuations"), on branch
`native-continuation`.

## What was done

A user `__next__` that calls an external or OS function used to fail with
`NotImplementedError: ... cannot suspend in this synchronous context` under `list()`, `tuple()` and `sum()`.
These consumers now park a serializable Rust continuation on the `__next__` frame instead of running it in a nested
`vm.run()`, so the suspension leaves only the main loop on the Rust stack and the session can be dumped.

`for` loops and comprehensions were part of this branch at first. Upstream #1384 (merged 2026-11-23) made them
suspend with its own mechanism (a stepping `py_next` returning `CallResult`, plus a frozen catcher frame), so this
branch now uses that and keeps only the consumers #1384 left synchronous. See "Rework onto #1384" below.

- `crates/monty/src/bytecode/vm/native_cont.rs` (new): `NativeCont` (`Collect`, `Sum`), the trampoline
    `run_native`, `start_native` for builtins, `resume_native` for the return/unwind hooks, and
    `finish_native_synchronously` for Rust callers that cannot yield.
- `CallFrame.continuation: Option<Box<NativeCont>>`, serialized as `K` on `SerializedFrame` (and on
    `SerializedTaskFrame`) with `#[serde(default, skip_serializing_if)]`, so dumps with no live continuation are
    byte-identical to before. Upstream already bumped `DUMP_VERSION` to 17 for #1384 in this release cycle, which
    also covers this field: a v16 release refuses these dumps rather than silently dropping a continuation.
- Hooks: `ReturnValue` and `handle_exception_step` hand the popped frame's outcome to its continuation. Only
    `StopIteration` is intercepted; any other exception keeps unwinding with the same exception object. Released in
    `cleanup_frame_state`, `cleanup_current_task`, `Task::drop_with` and `VMSnapshot::abandon`.
- `CallResult::finish_synchronously` takes a continuation off a just-pushed frame and drives it synchronously, so
    `evaluate_function`/`require_synchronous` callers (`map(tuple, ...)`, `sorted(key=...)`) keep working.
- Consumers: `list()` and `tuple()` (now `Constructor::Call`), and `sum()` (now returns `CallResult`).
- Docs: `limitations/iter.md` and `limitations/classes.md` say which consumers can now suspend.

## Rework onto #1384 (2026-11-24)

Merged upstream `cfa9cec8`, which includes #1384 "make iteration suspendable".

- Dropped our `NativeCont::ForIter` and the `ForIter` opcode hook; the opcode is upstream's version unchanged.
    The `for` and comprehension sections of `ext_call__native_continuation.py` stay as regression tests and pass on
    upstream's mechanism, including the dump/reload at every host call.
- Dropped the local `DUMP_VERSION` bump: upstream is already at 17.
- Regenerated the native-continuation fixture as `native_continuations_v17_suspended.bin`: the old one held a
    `ForIter` continuation, which no longer exists. Only this unmerged branch ever wrote it. The same scenario now
    dumps #1384's catcher frame (`for`) between two `NativeCont`s (`sum`, `list`).
- `resource_limits::max_turn_duration_restarts_each_resume` failed in 2 of 4 runs while other cargo builds were
    running, and passed 10/10 on an idle machine. It is a wall-clock test with a 50 ms per-turn budget; I treat it
    as a load flake, but it runs `sum(range(1_000))`, which goes through the refactored `sum.rs`.
- `tests/main.rs`: kept upstream's `external_function_in_next_suspends` (`for`) and our `set()` version of
    `external_function_in_next_raises_not_implemented` side by side.
- Limitations pages merged and re-checked against a probe run (Monty vs CPython): `list()`/`tuple()`/`sum()` over a
    user iterator suspend; `set()`, `list(map(...))`, `sum(map(...))` and `list()` over a suspending `__iter__` still
    raise `NotImplementedError`; `for` over `map(...)` and over a suspending `__iter__` suspend (#1384).
- Open design question for the maintainers: the interpreter now has two continuation mechanisms, #1384's catcher
    frame for `ForIter` and `NativeCont` for builtins that return a value. Whether builtins can reuse #1384's
    mechanism, or whether `NativeCont` should absorb `ForIter`, is raised in the upstream PR rather than decided here.

## Earlier merge with upstream (2026-11-23)

The fork's `main` was fast-forwarded to upstream `5f4a3dfc` (v1.7.0-beta.1), and that was merged into this
branch. The only textual conflict was `builtins/sum.rs`, but it hid a semantic one. Upstream #1386 switched
`sum()` to CPython's compensated float summation, while `NativeCont::Sum` still added items one by one with
plain `+`. After the merge, `sum()` over a suspending user iterator of floats would have differed from CPython
(`0.9999999999999999` instead of `1.0` for ten `0.1`s).

- The per-item step is now `Phase::fold` (`builtins/sum.rs`), shared by the normal loop and the continuation.
- `NativeCont::Sum` carries the `phase` (re-exported as `SumPhase`), so a compensated total survives a dump.
    `Phase` and `CompensatedSum` became `Serialize`/`Deserialize`, so their variant names are now dump format.
- New differential cases sum floats, complex numbers, mixed numbers, lists, and a float sum interrupted by a
    `str`, all from a suspending iterator.
- `native_continuations_v17_suspended.bin` was regenerated, since `phase` is a required field. Only this
    unmerged branch had ever written that fixture, so no released dump has the old shape.

## Tests

### Differential tests (CPython vs Monty)

The `crates/monty/test_cases` harness runs every case on both CPython 3.14 and Monty and requires the same
outcome. With `call-external = true` it also dumps and reloads the session at **every** host call, so each
suspension inside a continuation goes through the serialized format.

- `ext_call__native_continuation.py`: `for` (break / continue / else / nested / comprehensions / inside a
    function / early `return`), `list()`, `tuple()`, `sum()` (with and without `start`, the `TypeError` from a bad
    `start`), partly consumed iterators, errors propagating with their identity intact, errors caught inside
    `__next__`, `finally` around the suspension, continuations nested inside other continuations' callbacks, and
    synchronous boundaries (`sorted(key=...)`, `map(tuple, ...)`) still driving them.
- `ext_call__native_continuation_tb_for.py`, `ext_call__native_continuation_tb_list.py`: full tracebacks through
    a suspended `__next__`.
- `ext_call__native_continuation_async.py`: `list()`, `for`, `sum()` and `tuple()` over suspending user
    iterators inside three `asyncio.gather` tasks, with awaits between them.
- `dump_compat.rs::native_continuations_fixture_loads_and_resumes`: a checked-in dump suspended three
    continuations deep (`sum` → `for` → `list`) loads and finishes, and is re-dumped at each later suspension.

### Results (macOS, after the #1384 rework, 2026-11-24)

| Suite                                                                            | Result                            |
| -------------------------------------------------------------------------------- | --------------------------------- |
| `make lint-rs` (clippy + import check), `make lint-py`                           | clean                             |
| `cargo run -p monty-datatest` (all test_cases, CPython and Monty)                | 1598 passed, 0 failed             |
| `cargo test -p monty` (every binary except `heap_reader_compile_fail`)           | all pass                          |
| `cargo test -p monty --test heap_reader_compile_fail`                            | 3 fail, toolchain only: see below |
| `memory-model-checks`: native-continuation test_cases (4 files × 2 interpreters) | 8 passed                          |
| `memory-model-checks`: `--test dump_compat`, `--test main`                       | 18 and 30 passed                  |

I did **not** run the full `make test-memory-model-checks` (CLAUDE.md: very slow, CI runs it). One background run
of a subset exceeded the 1-hour limit, because a panic-in-panic left the process hung (see "Fixed during
verification").

Environment notes, so the numbers can be reproduced:

- The datatest binary had been linked against pyenv's Python 3.11 (`otool -L`), which made ~200 CPython
    comparisons fail. Build it with `PYO3_PYTHON=$PWD/.venv/bin/python` (3.14).
- With a non-UTF-8 locale, 10 CPython-side cases fail with `'ascii' codec can't decode`. Run with
    `LC_ALL=en_US.UTF-8`.
- `heap_reader_compile_fail` (3 tests) fails only on rustc diagnostic wording ("`x` is only valid" vs
    "`x` is a reference that is only valid", plus a macro-backtrace note) from the local nightly toolchain. This
    branch does not touch `heap/` or those tests. I deleted the generated `.snap.new` files rather than accept them.
- Not run: `make test-py`, `make test-js`, `make test-wasm`, subprocess/pool tests. The change is confined to the
    interpreter crate and adds no wire/API surface, but CI should confirm.

### Fixed during verification

- `native_continuations_fixture_loads_and_resumes` (written in the earlier session) panicked under
    `memory-model-checks` with `Value::Ref dropped without calling drop_with()`, then hung on a panic during
    unwinding. The cause was the test, not the VM: after re-dumping, it *dropped* the old suspended `ReplProgress`.
    Plain `Drop` does not release values held in snapshot frames; the datatest harness compiles out its dump
    round-trip under `memory-model-checks` for the same reason. The test now discards the old progress with
    `into_repl()`, which goes through `VMSnapshot::abandon`. That also puts continuation release on abandon under
    the refcount checks, and it passes.
- An early `make test-cases` run aborted with `fatal runtime error: stack overflow`. That build was linked
    against Python 3.11. Every later run against 3.14 completed all cases with no overflow, so I did not chase
    it further.

### Existing tests changed (please check this is acceptable)

Two tests in `crates/monty/tests/main.rs` asserted the old behaviour that this branch removes on purpose
(`for` / `list()` over a host-calling `__next__` raising `NotImplementedError`). Neither assertion was weakened.
Each now uses `set()`, which is still synchronous, so they keep testing what they were written for (synchronous
consumers reject host calls and restore state):

- `external_function_in_next_raises_not_implemented`: `for _x in Foo()` → `set(Foo())`. Same snapshot except the
    call-site line.
- `synchronous_iteration_rejection_restores_state`: `list(RecoverNext()) == []` → `set(RecoverNext()) == set()`.
    The other `list(...)` lines are unchanged and still raise, because `FailIter` fails in `__iter__` and
    `DirectNext.__next__` is a `functools.partial`, which has no frame to park on.

Two test_cases from the earlier session were also corrected, because they encoded divergences that are not
caused by this branch:

- `tb_list` used `return list(Bad())`. CPython hides carets when a frame's location is the whole `return` value
    (documented in `limitations/exceptions.md`), and Monty always draws them. It now uses `return len(list(Bad()))`,
    which both underline the same way. The frames under test are the same.
- I first added `StopIteration`-subclass and generator sections, then removed them: Monty cannot subclass
    builtin exceptions and does not support `yield`, both already documented.

## Deliberately not done (from the proposal)

- `pending_native` (a continuation parked beside `pending_effect` when `__next__` itself is a host function, so the
    call suspends without a frame). Those calls still go through `require_synchronous` and raise the old
    `NotImplementedError`. Not needed for `__next__` defined with `def`.
- Multi-element chains (`NativeChain { conts, landing }`): with only step-1 consumers every chain has one element,
    so a frame holds a single `NativeCont`, and `ForIter` does its own landing. Steps 2/3 (`map`/`filter`/`zip`
    stepping, key functions) will need the chain and the per-continuation `recursion_depth` charge. Today every
    continuation sits on a frame that already charges one level, so nesting is bounded.
- Steps 2-4: `set()`, `dict()`, `min`/`max`, `any`/`all`, `sorted`, adaptors, user `__iter__` in `for`.

## Where I got stuck / decisions for you

Nothing blocked the step-1 scope. These need a decision from you:

1. **`DUMP_VERSION` (resolved).** Upstream bumped it to 17 for #1384 in the same release cycle, which also makes
    dumps with a live `K` continuation unreadable by v16 releases, as the dump policy requires. No extra bump.
1. **Pre-existing traceback bug found (not fixed, out of scope).** When an exception passes through an `except`
    that does not match it, Monty reports the frame at the handler's last line rather than where the exception
    was raised. It reproduces on plain code with no continuation:
    `try: return undefined() / except KeyError: raise StopIteration` shows `raise StopIteration`, while CPython
    shows `return undefined()`. Worth its own issue.

## Needs careful human review

- **`sum()` phase inside the continuation.** While a float or complex phase is active, the `total` stored in
    `NativeCont::Sum` is stale and the real value is in `phase`. `exhausted` stores it back and the error path
    drops `total`. Check that no path reads `total` without `store` first.
- **Refcount ownership in `native_cont.rs`.** `NativeCont` owns its `iterator`, `items` and `total` across
    frames, dumps and Rust-stack hand-offs. Every error branch in `next_item` / `resume` / `run_native` drops it
    manually. Check that no branch drops twice or leaks, especially `Sum` when `py_add` fails, and `Collect` when
    `check_growth` fails. Also check the four places a frame can be discarded (`cleanup_frame_state`,
    `cleanup_current_task`, `Task::drop_with`, `VMSnapshot::abandon`).
- **Exception unwinding hook (`exceptions.rs`).** On `StopIteration` the continuation resumes in the caller frame.
    Any new error it raises comes back as `ExceptionHandlingResult::Raised` and is handled from scratch. Any other
    exception drops the continuation and keeps unwinding the *same* exception object. The traceback position comes
    from restoring `instruction_ip` from the popped frame's `call_offset`. Check that it is correct when the
    consumer is several frames down.
- **`ReturnValue` hook (`vm/mod.rs`).** It asserts that a continuation frame is never an initializer or a
    `should_return` frame. `run_native` avoids parking on initializer frames by running them synchronously
    (`__next__` naming a class). Please confirm that no other `should_return` frame can carry a continuation.
- **`finish_synchronously` with a parked continuation (`call.rs`).** This is how `sorted(key=...)` and
    `map(tuple, ...)` still work: the continuation is taken off the just-pushed frame and driven in a nested run.
    It runs under the caller's re-entry reservation. Check the recursion and re-entry accounting.
- **Resource limits.** Items produced without a frame loop inside `run_native` and poll `check_time_every`.
    Items from frames go through the main loop's checkpoints. `Collect` calls `check_growth` per item.
- **Untrusted dumps.** A tampered `NativeCont` (for example `ForIter` with a non-instance on the stack) yields
    `RunError::internal`, not UB, which matches the snapshot trust contract. Please confirm no panics are
    reachable that would matter.
- **Async tasks.** Continuations move with frames into and out of `SerializedTaskFrame` (`async_exec.rs`).
    `ext_call__native_continuation_async.py` runs continuations inside gathered tasks. But a continuation is only
    live while a synchronous `__next__` runs, and that cannot `await`, so I believe no test (and no program) can
    switch tasks while one is live. That makes the `async_exec.rs` / `SerializedTaskFrame` plumbing defensive and
    untested. Please confirm that reasoning or point at a path that reaches it.
