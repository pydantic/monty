# Resource limits

Monty enforces hard limits on memory, time, and recursion to keep untrusted
code bounded. Memory limits surface to the host as terminal `MemoryError`s,
while time limits surface as terminal `TimeoutError`s; sandboxed code cannot
catch them. `RecursionError` is catchable, as in CPython.

## Compilation

`ResourceLimits` starts when the VM is created; parsing, preparation, and
bytecode compilation are not charged to its memory or duration budgets.
Compilation has separate structural caps for parser nesting, bytecode operand
sizes, comprehension nesting, and repeated `finally` expansion. In particular,
a code object requiring more than 1,024 emitted copies of `finally` bodies is
rejected with `SyntaxError`; CPython has no equivalent limit. Production hosts
should still isolate compilation when accepting untrusted source, as the
subprocess and WebAssembly runtimes do.

## Memory / size limits

- Memory tracking is global; the host sets the bytes budget when
  constructing the VM.
- The byte count is **approximate**: per-object sizing uses `py_estimate_size`,
  which elides bookkeeping overhead (HashMap bucket padding, `Vec` capacity
  slack, `SmallVec` inline buffers, scheduler queue allocations) and rounds
  per-spawn task overhead to a fixed conservative constant. The configured
  `max_memory` is a budget on user-visible data, not a hard ceiling on
  process RSS.
- Operations whose result is bounded by simple arithmetic on input sizes
  are **pre-checked** before allocating: integer multiplication, left
  shift, integer power, sequence repeat (`'x' * n`), replacement
  (`str.replace`, `bytes.replace`), padding (`str.ljust`, `str.center`,
  `str.zfill`, `bytes.ljust`, …), and f-string formatting
  (both dynamic width `f"{v:>{w}}"` and dynamic precision on float
  formats `f"{v:.{p}f}"` / `e` / `%`). The pre-check threshold is 100 KB —
  estimates above that are checked against the remaining budget and rejected
  with `MemoryError` before allocation when they would exceed it.
- `bigint.pow(base, exp)` estimates result size as `bits(base) * exp` with
  a 4× safety multiplier to cover repeated-squaring intermediate values.

## Exceeding `max_memory` in a worker (pools)

A worker enforces `max_memory` in its own global allocator as well, counting
every byte the process requests. Nothing to configure: setting `max_memory` on a
session applies it, and a session without one is unlimited. Whichever of the two
enforcement paths binds first decides how exceeding the limit surfaces, so both
outcomes below are reachable for the same `max_memory`.

<!-- TODO: update once there's one memory-limit implementation — the two paths
below collapse into the allocator's, and only its outcome remains. -->

- **The allocator's outcome kills the worker, though it still reports
  `MemoryError`.** Exceeding the limit fails an allocation the interpreter
  cannot handle, so the worker exits mid-turn. The host gets
  `PoolError::Runtime` / `MontyRuntimeError` wrapping a `MemoryError` — but
  unlike every other runtime error, and unlike the interpreter's own
  `MemoryError`, the session is gone with the worker (later calls on that
  checkout report `Finished`; the pool itself recovers). Sandboxed code cannot
  observe or catch it.
- **Which one you get depends on what the code allocated.** The interpreter
  measures user-visible data (see the approximation note above) while the
  allocator measures what was really requested, so the same `max_memory` binds
  earlier for many small objects than for a few large ones. Workloads dominated
  by small objects tend to end the worker; workloads dominated by large buffers
  tend to raise the interpreter's catchable-by-the-host `MemoryError` and leave
  the session alive.
- **It binds the worker's allocator, not the process.** Only bytes requested
  from Rust's global allocator are counted, which is everything sandboxed code
  can cause to be allocated, but not memory obtained another way: thread stacks,
  the binary's own mapped image, or a direct `mmap`. It is not a kernel-enforced
  bound on process memory — an inherited `ulimit -v` or cgroup limit is the tool
  for that, and still applies independently (a worker whose allocation the
  kernel then refuses reports the same `MemoryError`).
- **It counts requested bytes, not resident ones.** Per-allocation overhead and
  fragmentation sit between the count and the process's real footprint, so RSS
  runs somewhat above the limit.
- **`max_memory` alone does not bound worker memory.** Above the limit sits the
  worker's own footprint plus fixed headroom for machinery a session never asked
  for — a few MiB, more when type checking loads typeshed and salsa. The
  headroom is deliberately generous: too tight a cap kills healthy workers.
  Use `max_processes` and an OS-level limit to bound a host, not this.
- **Per session, but against a fixed baseline.** A worker serves many checkouts
  and re-derives the cap for each session, always from the leanest the process
  has been. Memory retained between sessions therefore consumes the headroom
  rather than raising the cap, and a worker whose residue outgrows it is killed
  and replaced rather than allowed to grow indefinitely.
- **Restoring a dump is bounded by the checkout it lands in.** `load_session` /
  `load_snapshot` restore the dump's own limits (see
  `limitations/pool-architecture.md`), and the cap is re-derived from them
  once the session exists — but the load *itself* runs under the limit the
  `checkout()` config applied. Restoring a large dump into a checkout with a
  much smaller `max_memory` can therefore exceed it while loading; pass a
  comparable limit to `checkout()`.
- **The wasm worker enforces it but cannot report it.** It applies the same
  limit in the same allocator, and exceeding it traps the instance — but a wasm
  module has no exit status, so the host reports `MontyCrashedError` rather than
  the `MemoryError` a subprocess produces. Its `usize` is also 32 bits, so a
  limit near 4 GiB leaves the module uncapped.
- WebSocket workers get no allocator-enforced limit at all: they are remote
  processes this pool does not spawn.

Independently of any limit, **any** allocation a worker's allocator refuses —
plain host OOM, or a request beyond the usable address space such as
`' ' * (1 << 60)` — takes this same path on every platform: the worker exits and
the host sees that `MemoryError` with its session gone. CPython raises a
catchable `MemoryError` in-process and carries on. Monty cannot: the failure
happens below the interpreter, where no Python-level exception can be raised, so
the worker classifies the failure into a dedicated exit code and dies. (Without
that, the process would abort with `SIGABRT` — indistinguishable from a stack
overflow, which is why the sandbox exits deliberately instead.)

## Integer-specific caps

- `pow(base, exp)` / `base ** exp` with an exponent larger than `u32::MAX`
  (≈ 4.3 × 10⁹) raises `OverflowError: "exponent too large"`.
- `pow(base, exp, mod)` requires all integer arguments and rejects negative
  exponents (`ValueError`).
- `int(str_or_bytes, base)` rejects inputs over 4,300 digits before the
  potentially quadratic BigInt parse when the effective base is not a power
  of two. The fixed cap matches CPython's
  `sys.int_info.default_max_str_digits`.

## Recursion

- Python-level call depth is hardcoded at **1000 frames**. The 1001st
  nested call raises `RecursionError`.
- Production sandbox code cannot change the recursion limit. Test builds may
  expose `sys.setrecursionlimit()` as a lowering-only fixture hook; it cannot
  raise the host-configured ceiling.
- Async stacks count toward the limit but each `await` boundary is treated
  as one frame, so `await`-chains do not amplify depth.
- Callbacks evaluated synchronously by the interpreter itself re-enter on the
  native Rust call stack rather than the heap-allocated frame stack used by
  ordinary function calls. This includes `map()`, `filter()`,
  `sorted()`/`list.sort(key=...)`, `min()`/`max(key=...)`, recursive
  `__repr__`/`__str__`, and non-plain-function `__init__` values that recurse
  during construction. Native re-entry is capped independently at a lower
  fixed depth than the 1000-frame Python limit, so Monty raises
  `RecursionError` before a native stack overflow would abort the process. See
  `limitations/classes.md`'s `__repr__`/`__str__` entry for the main
  user-visible divergence this causes.

## Time

- The host can set a `max_duration` budget; if exceeded the VM stops on
  the next bytecode boundary with `ResourceError`.
- Enforcement is polled, not preemptive: a single bytecode instruction may
  run a long native operation (a `bytes` substring scan, a sort, an iterator
  drain), and those poll the clock at a coarse granularity. A run can
  therefore overshoot `max_duration` before stopping.
- `bytes` operations that search for a sub-sequence (`in` with a bytes-like
  probe, `find`, `count`, `split`, `partition`, `replace` and their
  variants) poll the clock every 64KiB, or every two lengths of the
  searched-for sequence if that is longer. Searching for a
  sequence over 64KiB therefore overshoots `max_duration` in proportion to
  its length, and `max_memory` does not bound that length: it caps sequences
  built at runtime, but a `bytes` literal is interned when the source is
  parsed and never counted against it.
- The neighbouring `bytes` operations that scan without a sub-sequence are
  **not** polled and run to completion however large the input: `in` with an
  integer probe (a single-byte scan) and `split()`/`rsplit()` left to their
  default `sep=None` (whitespace splitting).
- The budget covers cumulative **execution time**, not wall-clock time:
  the clock runs only while the interpreter executes bytecode, and is
  paused while execution is suspended waiting on the host (external
  function calls, OS callbacks) and between REPL feeds. It accumulates
  across feeds for the life of the session.
- The accumulated time is serialized into dumps/snapshots, so a restored
  session resumes its budget where it left off rather than restarting
  from zero.
- There is no in-sandbox way to observe the budget or remaining time.

## JSON

- `json.loads` rejects input nested deeper than 200 levels with
  `json.JSONDecodeError` (independent of the Python recursion limit).

## After a terminal resource error

After a memory or time limit fires, **no guarantees are made about
heap state or reference counts**. The host should discard the VM rather than
try to recover and continue running code in it. A caught `RecursionError` does
not invalidate the VM and execution may continue inside the sandbox.
