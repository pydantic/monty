# Performance analysis cookbook

How to find out *where* Monty spends time, and how to be confident a change
actually helped. Written after tracing a runtime regression in exception
unwinding; the pitfalls listed are ones that actually bit.

The order below is the order to work in. Steps 1–3 answer "is there really a
regression, and what shape is it?" and need no profiler. Only reach for step 4
once you know what you are looking for.

## 0. Build a binary that can be profiled

The `dev` profile produces a **stripped** binary on macOS: `sample`, `lldb`, and
`atos` all report `___lldb_unnamed_symbol_...` and the profile is useless. Use
the dedicated profile (release codegen, `debug = true`, `strip = false`,
`lto = false` so it links fast):

```bash
cargo build --profile profiling --bin monty     # -> target/profiling/monty
```

Use `target/debug/monty` for correctness work, `target/profiling/monty` for
anything timing-related. Note `MAX_NESTING_DEPTH` is 30 under `debug_assertions`
and 200 otherwise, so deeply-nested sources behave differently between the two.

## 1. Sweep a parameter — never measure a single point

One number tells you nothing. Generate the same workload across a parameter
(nesting depth, element count, iteration count) and look at the **shape** of the
curve: flat, linear, quadratic, exponential. That shape is the finding; the
absolute numbers are not.

```python
# gen_sweep.py — exception propagating through N nested finallys
def sweep(depth, iters=200_000):
    ind = lambda n: '    ' * n
    s = 'def f():\n'
    for i in range(depth):
        s += f'{ind(i + 1)}try:\n'
    s += f'{ind(depth + 1)}raise ValueError\n'
    for i in range(depth - 1, -1, -1):
        s += f'{ind(i + 1)}finally:\n{ind(i + 2)}pass\n'
    return s + f'\nfor _ in range({iters}):\n    try:\n        f()\n    except ValueError:\n        pass\n'

for d in (1, 2, 4, 8, 12, 16, 20, 24):
    open(f'sweep_{d:02d}.py', 'w').write(sweep(d))
```

```bash
for f in sweep_*.py; do
  printf '%s ' "$f"
  for i in 1 2 3; do /usr/bin/time -p target/profiling/monty "$f" 2>&1 | awk '/^real/{printf "%s ", $2}'; done
  echo
done
```

A flat curve where you expected linear (or vice versa) is the signal worth
chasing. In the case this document came from, the merge base was flat across
depth 1→24 while the branch was linear — that contrast was the entire lead.

## 2. Repeat every measurement — single runs lie

A one-shot reading produced `0.68s` for a case that is really `0.019s`; it was
scheduler noise from a background build and vanished on repetition. Always run
3–5 reps and read the **minimum** (least contaminated by other load), and never
report a delta you have not seen survive repetition.

```bash
for i in 1 2 3 4 5; do /usr/bin/time -p target/profiling/monty case.py; done 2>&1 | grep real
```

Keep the machine quiet: no background `cargo build` while benchmarking.

## 3. A/B against the merge base, in a worktree

Build the baseline once, in a throwaway worktree, and run both binaries over the
identical generated files:

```bash
BASE=$(git merge-base main HEAD)
git worktree add /tmp/mb $BASE
(cd /tmp/mb && cargo build --profile profiling --bin monty)
./playground/perf/bench.sh /tmp/mb/target/profiling/monty 3   # baseline
./playground/perf/bench.sh target/profiling/monty 3           # current
git worktree remove --force /tmp/mb
```

## 4. Isolate the mechanism with control cases

Before profiling, narrow *what* is expensive by varying one axis at a time and
keeping everything else fixed. Each control either reproduces or doesn't, and
that bisects the cause faster than any profiler. For the unwind regression:

| Variant | Result | Conclusion |
| --- | --- | --- |
| deep chain, returns normally | fast at any depth | not the nesting itself |
| deep chain, `try/finally` at each level | fast | not the depth of the chain |
| deep chain, exception with no handler | scales with depth | the unwind path is the cost |

## 5. Sampling profiler

macOS `sample` attaches to a running PID, so start the workload and sample it:

```bash
target/profiling/monty playground/perf/sweep_24.py & PID=$!
sleep 1 && sample $PID 4 -mayDie -file /tmp/prof.txt
grep -v "^ *$" /tmp/prof.txt | head -60          # call graph, heaviest first
```

To symbolise a bare address by hand (`Load Address` is printed in the header):

```bash
atos -o target/profiling/monty -l <load-address> <address>
```

For flamegraphs over the committed benchmark suite there is already a target:

```bash
make profile        # cargo bench --profile profiling -- --profile-time=10, then
                    # scripts/flamegraph_to_text.py
make bench          # criterion benchmarks (crates/monty-bench)
make dev-bench      # smoke-run the benchmarks without measuring
```

## 6. Peak memory

`/usr/bin/time -l` reports peak RSS on macOS. Watch it across a sweep the same
way as time — flat RSS over a long hot loop is how you prove the absence of a
leak (e.g. 200k iterations of exception-swallowing staying at 16 MB).

```bash
/usr/bin/time -l target/profiling/monty case.py 2>&1 | grep "maximum resident"
```

## 7. Crashes: lldb works even on a stripped binary

Repeated **identical return offsets** in a backtrace mean direct self-recursion,
which is readable without any symbols at all:

```bash
lldb --batch -o "run case.py" -o "bt 40" target/debug/monty
```

If every frame reads `... + 2204`, one function is calling itself — find the
single self-recursive call on that code path by grepping the source.

## 8. Then read the code for allocations

Profilers point at a function; they rarely explain *why* it is hot. Once you
know where the time is, read that path looking specifically for per-iteration
`clone()`, `heap.allocate(...)`, `Vec::new()`, and refcount churn. In the unwind
path, each nesting level ran `create_exception_value` (an `ExcInfo` clone plus a
`heap.allocate`) whose result was pushed to the operand stack and then
immediately discarded by a compiler-emitted `Pop` — invisible in a sampling
profile spread across allocator frames, obvious on inspection.

## Worked example

The loop above, end to end, on exception unwinding through nested `finally`:

1. **Sweep** showed cost linear in nesting depth, ~11.1 ms per level per 200k
   iterations — the shape said "per-level work", not "one slow thing".
2. **Profile** (step 5) put ~48% of samples in exception-object churn:
   `Heap::allocate`, `SimpleException::clone`, `ExceptionRaise::clone`,
   `dec_ref`/`drop_in_place`.
3. **Read** (step 8) explained why: each level ran `Value` → `RunError` →
   `Value`, freeing a heap exception and allocating an identical one.
4. **Fix**: carry the existing object through (`handle_exception_with_value`)
   instead of rebuilding it.
5. **Re-measure**: 11.1 → 7.6 ms per level (−32%); allocations 281 → 14 samples
   (−95%), `SimpleException::clone` −72%, `ExceptionRaise::clone` gone.

Note step 4 was the *second* attempt. The first — skipping the operand-stack
push for cleanup handlers — was a real reduction in work and showed up in the
profile, but moved wall-clock by **zero**, because it targeted refcount traffic
rather than the allocations. Measure the fix, not just the problem.

## Pitfalls

- **Stripped dev binary** — see step 0; this silently wastes a lot of time.
- **Background load** — a `cargo build` running during a benchmark fabricated a
  35× "regression" that did not exist.
- **Debug vs release limits** — `MAX_NESTING_DEPTH` (30 vs 200) and frame sizes
  differ, so crash thresholds and deep-nesting behaviour are not comparable
  between profiles. State which profile a number came from.
- **Compilation is not charged to `ResourceLimits`** — when a workload looks
  slow, check whether the cost is compile-time or run-time before optimising the
  wrong phase. Split the source into a compile-only case (never call the
  function) to tell them apart.
- **Don't report a mechanism you haven't confirmed.** A plausible story that
  matches the curve is still a guess until a control case or the source
  confirms it.
