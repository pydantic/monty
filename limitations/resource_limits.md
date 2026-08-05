# Resource limits

Monty enforces limits on memory, time, and recursion to keep untrusted code
bounded. Memory limits surface to the host as `MemoryError`s and time limits as
`TimeoutError`s; sandboxed code cannot catch either resource error.
`RecursionError` is catchable, as in CPython.

## Compilation

`max_duration` starts when the VM executes, so parsing, preparation, and
bytecode compilation do not consume it. In workers, allocations retained by
compiled code do count toward `max_memory`; transient compilation allocations
are released before execution reaches its first memory checkpoint.
Compilation has separate structural caps for parser nesting, bytecode operand
sizes, comprehension nesting, and repeated `finally` expansion. In particular,
a code object requiring more than 1,024 emitted copies of `finally` bodies is
rejected with `SyntaxError`; CPython has no equivalent limit. Production hosts
should still isolate compilation when accepting untrusted source, as the
subprocess and WebAssembly runtimes do.

## Memory / size limits

- Memory usage is measured by the worker's process-global allocator, while the
  configured budget belongs to one session.
- Workers count bytes requested from their global allocator. Direct Rust users
  must install and arm `monty-alloc` before using `max_memory`; otherwise
  execution fails with `RuntimeError` rather than applying an approximate limit.
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

A worker counts every byte requested from its global allocator. Nothing extra is
enabled by the host: setting `max_memory` on a session applies it, and a session
without one is unlimited.

- **The configured limit is soft.** The interpreter reads current allocator
  usage at execution checkpoints and reports a terminal `MemoryError` to the
  host after crossing it. The incomplete operation is unwound and the worker
  and session survive, although sandboxed Python cannot catch resource errors.
- **A burst can still kill the worker.** A hard ceiling sits above the configured
  limit so exception and traceback machinery can run. Crossing that ceiling
  between checkpoints exits the subprocess with its dedicated OOM status, or
  traps wasm. The pool replaces the worker and the session is lost. Large
  result operations are pre-checked to avoid this path when their size is known.
- **Work outside Python execution is hard-limit-only.** Request framing, input
  decoding, loading snapshots, and type checking do not reach an interpreter
  checkpoint. A sufficiently large allocation there can therefore cross the
  hard ceiling and kill the worker.
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
- **`max_memory` alone does not bound worker memory.** The hard ceiling includes
  the worker's baseline plus a fixed gap above the soft limit — a few MiB, more
  with type checking. Use `max_processes` and an OS-level limit to bound a host.
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
- **The wasm worker cannot classify a hard breach.** A soft breach is a normal
  `MemoryError`, but exceeding the hard limit traps the instance and the host
  reports `MontyCrashedError`. Its `usize` is also 32 bits, so a limit near
  4 GiB leaves the module uncapped.
- WebSocket workers get no allocator-enforced limit at all: they are remote
  processes this pool does not spawn.

Independently of any limit, **any** allocation a worker's allocator refuses —
plain host OOM, or a request beyond the usable address space such as
`' ' * (1 << 60)` — takes this same path: on a worker with an exit status the
host sees that `MemoryError` with its session gone, and on wasm the same
refusal traps, reported as `MontyCrashedError` per the bullet above. CPython
raises a catchable `MemoryError` in-process and carries on. Monty cannot: the failure
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
  its length.
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

A worker remains responsive after a soft memory or time limit and its session
can receive another feed, but execution is not transactional and no guarantees
are made about heap state or reference counts. Hosts should discard the session;
the worker itself remains reusable. A caught `RecursionError` may continue
normally inside the sandbox.
