# Plan: Close the Monty ↔ CPython pure-bytecode performance gap

## Context

Monty is ~1.5× slower than CPython (geomean over the 21 paired benchmarks). The gap is
concentrated in agent-workload-shaped code: regex extraction (re_extract 3.4×), dict-heavy
aggregation (agg_rows 2.8×), function calls (fib 2.3×, func_call_kwargs 4×), string parsing
(str_parse 2.2×). Startup/parse and Rust-backed builtins already beat CPython. Agent code is
wall-to-wall method calls, dict access, and string handling, so these are the paths worth
optimizing.

Evidence: perf-insights.txt (local pprof) + CodSpeed run `6a4a8d2ea5068cefef997792`
(main @ f576e90) flamegraphs via the CodSpeed MCP, cross-checked against source by three
exploration passes.

**Headline finding that revises perf-insights.txt:** re_extract's 3.4× gap is almost
entirely Monty-side waste (subject-string copies, eager char counting, regex recompilation),
not regex-engine internals — it was previously written off as unfixable. It is now the
cheapest big win. Conversely the vectorcall idea, while still the right structural change
for call-heavy code, will NOT move agg_rows (dict-method calls via `CallAttr`, and
`sorted(key=lambda)` via `evaluate_function` — neither uses `exec_call_function`) nor
func_call_kwargs (kwargs take the slow path; that microbench is mostly per-run setup cost).

## Flamegraph evidence (main @ f576e90)

| Benchmark | vs CPython | Dominant costs |
|---|---|---|
| re_extract 39.4ms | 3.4× | `ReMatch::from_captures` **56%** (full-subject memcpy per match 32% + eager byte→char `chars().count()` scans 20%); `re.split` recompiling `\s+` per loop iteration **19%** (and each compile builds 3 regexes) |
| agg_rows 59.4ms | 2.8× | `dict::find_index_hash` **26%** (clones+drops candidate key per probe to dodge borrowck); `clone_with_heap` 6%; `dec_ref` 6.6%; `format!`-based str concat in `py_add` 6.6%; `HeapReader::read` reader-count tax 4.6% |
| fib 163.6ms | 2.3× | call scaffolding >40%: `call_def_function` 25%, `Signature::bind` 9%, `install_closure_cells` 3%, plus arg memmove traffic; `load_local` 8% |
| str_parse 39.4ms | 2.2× | `call_str_method_impl` 34%; malloc/free ~20% + dec_ref churn ~16% from ~20k short-lived heap `Str` temporaries |
| func_call_kwargs 22µs | 4× | mostly per-run interpreter setup (hashmaps, scheduler, heap teardown); kwargs binding is a slice of it |

---

## Track 1 — `re` module fixes (re_extract; expect ≥2× on this bench; low risk) — ✅ DONE

All five items below landed, measured ~2.5× on re_extract locally. Beyond the plan: the
pattern cache is
memory-bounded (256 slots × 64 KiB `delegate_size_limit`, oversize patterns compile
uncached — closes a resource-limit escape), size-limit compile errors are distinguished
from syntax errors so invalid patterns compile only once, the group-0 char span is
memoized per match, and `m.string is subject` now matches CPython (divergence removed).

Files: `crates/monty/src/types/re_match.rs`, `types/re_pattern.rs`, `modules/re.rs`.

1. **Stop copying the whole subject (and pattern) into every `ReMatch`**
   (`from_captures`, re_match.rs:126 — `input_string: input.to_owned()` = 32% of the bench;
   1500 full-subject copies in the benchmark). Share the subject across matches from one
   `finditer`/`search` call — e.g. `Arc<str>`/`Rc<str>` field, or a refcounted heap `Value`
   pointing at the original `Str`. `ReMatch` is serde-serialized for snapshots: serializing
   the shared subject per match is acceptable (dedup is a non-goal). `pattern_string` is
   only used by `repr` — same treatment.
2. **Lazy byte→char offset conversion** (re_match.rs:91–103; `byte_to_char_offset` at
   488–490 rescans from byte 0, ~6× per match = 20% of the bench). Store byte offsets;
   convert only when `.start()/.end()/.span()` is actually called. Fast paths: ASCII
   subject ⇒ byte==char; when converting several offsets, one forward scan.
3. **Compiled-pattern cache for module-level `re.*` calls** (`resolve_pattern`, re.rs:632
   recompiles the pattern string on every call; CPython caches 512). Bounded cache keyed on
   (pattern, flags).
4. **Lazily compile the anchored `match`/`fullmatch` regex variants**
   (`RePattern::compile`, re_pattern.rs:83–94 builds 3 regexes eagerly; `re.split` uses
   only 1). `OnceCell` the `\A(?:…)` and `\A(?:…)\z` variants.
5. **Zero-copy subject in Pattern-method dispatch** (re_pattern.rs py_call_attr ~347–372
   does `.to_str().to_owned()` on the full subject; the module-level path already borrows
   via `subject_str`, re.rs:485).

## Track 2 — dict lookup without candidate-key clone (agg_rows; attacks ~35%; medium risk)

File: `crates/monty/src/types/dict.rs` (`find_index_hash`, 481–510).

Every probe currently does `candidate_key.clone_with_heap` + `defer_drop` + `py_eq(&mut vm)`
— the clone exists only to satisfy the borrow checker. For key types whose equality cannot
run arbitrary Python (str — `InternString` or heap `Str` — plus int/bool/float/bytes),
compare directly against the borrowed stored key without cloning and without `py_eq`,
following the existing in-file precedents `get_by_str` (dict.rs:271) and
`json_key_equals_str` (dict.rs:214). Keep the clone+py_eq path as fallback for instances /
reflected-protocol cases. Benefits `getitem`, `setitem`, and `dict.get` alike (all funnel
through `find_index_hash`), and removes the matching share of `clone_with_heap`, `dec_ref`,
and `HeapReader::read` reader-count traffic.

## Track 3 — string cheap wins (agg_rows, str_parse, fstring_report; trivial risk)

1. **Direct string concat in `py_add`** (value.rs:450–462): replace `format!("{}{}", a, b)`
   with `String::with_capacity(a.len()+b.len())` + 2× `push_str`, mirroring the bytes arms
   at 464–480. Result size is bounded by already-tracked inputs, so plain `String` is fine
   per the StringBuilder rule. ~6.6% of agg_rows, ~3% of str_parse.
2. **Hoist the wasted `DefaultHasher` init in `Value::py_hash`** (value.rs:1626) — built
   unconditionally but unused by the hot Int/InternString/Ref arms.
3. **Borrow instead of `.to_owned()` for `startswith`/`endswith` prefix args**
   (str.rs `parse_prefix_suffix_args` ~1117–1154).
4. **`dec_ref` work-stack `Vec::new()` per call** (heap.rs:1015): SmallVec or a pooled
   stack. `heap.rs` is a security/soundness-critical file — smallest possible diff, or
   defer if the review cost outweighs the win.

## Track 4 — vectorcall-style in-place args (fib and all plain `def` calls; highest effort)

Files: `crates/monty/src/bytecode/vm/call.rs`, `vm/mod.rs`, `args/bind_python.rs`,
`function.rs`.

For `Opcode::CallFunction` (all-positional) targeting a sync def-function with
`BindMode::Simple`/`SimpleWithDefaults`: leave the args on the operand stack and turn them
into the callee's locals in place, eliminating the stack→`ArgValues`→scratch→stack round
trip. Design validated against unwind, snapshots, and the resource tracker:

- **Check** at the top of `exec_call_function` (call.rs:113), before `pop_n_args`: peek the
  callable at `stack[len-argc-1]`; `DefFunction(fid)` directly, or `Ref` → one `heap.get`
  discriminant match for `Closure`/`FunctionDefaults`; one `interns.get_function` Vec index
  gives `is_async` + `bind_mode` + counts. No new opcode needed (callee unknowable at
  compile time; `CallFunction` already guarantees contiguous positional args).
- **Callable slot**: `stack.remove(callable_idx)` + `defer_drop` (an O(argc≤4) memmove) so
  `stack_base = len - argc` and **zero changes** to `CallFrame`, serialization,
  `cleanup_frame_state`, or `handle_exception` — every existing drain/truncate invariant
  holds. (Alternative `owns_callable: bool` on the frame documented as a follow-up only if
  the memmove ever shows up.)
- **Frame setup**: charge tracker `namespace_size * VALUE_SIZE` (identical accounting to
  today — resource-limit behavior unchanged); push defaults suffix cloned straight from the
  heap `FunctionDefaults`/`Closure` (also kills today's per-call defaults Vec clone in
  `call_heap_callable`, call.rs:458); push `Undefined` to `namespace_size`; new
  `install_closure_cells_at(func, cells, base)` writing `stack[base+slot]` (heap.allocate
  is `&self` — verified borrowable; don't hold a `heap.get` borrow across `allocate`).
- **Arity mismatches never raise from the fast path** — bail to the slow path so error
  construction stays in one place (`wrong_arg_count_error`).
- **Out of scope**: bound methods (`self` isn't below the args on the stack) — noted
  follow-up: CPython-style `LoadMethod`/`CallMethod` opcodes; kwargs fast path; async.
- **Increments** (each keeps the suite green):
  0. Cache `param_count`/`required_positional_count` as `Signature` fields (bind currently
     recomputes via `Option<Vec>::len` chains per call) + `is_simple_positional()` accessor.
  1. Fast path for `Value::DefFunction`, no cells (covers fib).
  2. Extend to heap `FunctionDefaults`/`Closure` without cell/free vars.
  3. In-place cells via `install_closure_cells_at`; closures fully covered.
- Expected: fib −20–35%; kitchen_sink and agent benches proportional to `def`-call density;
  agg_rows/func_call_kwargs ~unchanged (different call paths — see Context).

## Track 5 — dispatch-loop micro-wins (broad, small)

1. **`should_gc` per-instruction** (heap.rs:1121, called every dispatch at vm/mod.rs:943):
   once `purple_count > 0` the early-out stops helping and every instruction reads
   `gc_interval()` + compares counters. Cache the interval in a field; better, move the
   check to allocation points (the counter only changes there), keeping a per-instruction
   check only if required for timely collection.
2. Defer deeper loop work (computed goto, opcode fusion, LoadMethod/CallMethod) until after
   Tracks 1–4 land and CodSpeed shows the new profile.

## Track 6 — char-counting strategy for `str` (broad; scope after Tracks 1–3 re-profile)

`chars().count()` is O(n) and appears at ~40 call sites, several hot: `str.find`/`index`/
`count`/`rfind` recount the whole string on every call just to normalize `start`/`end`
args (str.rs:926–1038, inside the `call_str_method_impl` path that is 34% of str_parse);
`len()` (str.rs:193, value.rs:151); string indexing/unpacking in the VM
(vm/collections.rs:532/565); `ljust`/`rjust`/`center` (str.rs:1727–1848); `ord`; file
reads. Two levers, in value order:

1. **Cache the char count on heap `Str`** — `Str` already lazily caches its hash in a
   `Cell<Option<HashValue>>` with `#[serde(skip)]` (str.rs:38); a char count (or at least
   an `all_ascii` bit, mirroring `ReMatch.all_ascii`) is the identical pattern and makes
   `len()`/indexing/position methods O(1) after first touch. Interned strings could
   precompute at intern time. This is the algorithmic win; costs 8 bytes per string.
2. **`bytecount::num_chars` for the remaining genuine scans** — real SIMD for the same
   non-continuation-byte count (aarch64 NEON on by default; x86 needs its
   `runtime-dispatch-simd` feature; the wasm32 path emits simd128 — verify `make
   test-wasm` before adopting). Honest sizing: std's `Chars::count` is already SWAR
   word-at-a-time and strings <16 bytes take bytecount's scalar fallback, so this is a
   2–6× constant factor on long strings only — a supplement to lever 1, not a substitute.

Also covered by lever 1: the `write_text`/`append_text` return values (fs/common.rs:61/82,
fs/overlay.rs:305/342) recount content that `os.rs::extract_str_data` (os.rs:488) had in
hand as a heap `Str` moments earlier — with a cached count the natural shape is to take
the count at extraction and stop counting in `fs/` entirely. Low priority on its own
(I/O-amortized, CPython-mandated count), but it falls out of the same change.

## Deliberately skipped

- str_parse's residual `to_lowercase`/`trim` result allocations — inherent to
  immutable-string semantics; CPython's edge there is its object free list (a much bigger
  architectural change, revisit only if str_parse still lags after Tracks 2/3).
- Regex engine tuning beyond Track 1 — after it, the remaining time is genuine matching.
- func_call_kwargs-specific work — the bench mostly measures per-run setup; revisit with a
  kwargs-heavy loop benchmark if kwargs binding matters in practice.

## Verification

- Per track: `make bench` locally (paired CPython comparison), then land as its own PR —
  CodSpeed CI gives per-benchmark deltas; quantify with CodSpeed MCP `compare_runs`.
- `make test-cases`, `make test-memory-model-checks` (the critical soundness gate for
  Tracks 2 and 4), `make test-ref-count-return`, `make test-no-features`, `make main`.
- Track 4 specifically: `function__arity_defaults.py` (error parity), `closure__*.py` /
  `class__closures.py` (cell layout), `ext_call__in_function.py` / `ext_call__recursion_bug.py`
  / `tests/binary_serde.rs` (snapshot suspend/resume inside fast-path frames),
  `execute_raise__*` / `tests/error_locations.rs` (unwind + tracebacks),
  `tests/resource_limits.rs` (tracker symmetry).
- No user-visible behavior changes intended ⇒ no `limitations/` updates expected; verify re
  semantics (match object attributes, span values) stay CPython-identical via test_cases.

## Landing order (one PR per track)

1. **Track 3** — half a day, immediate visible wins on agg_rows/str_parse/fstring.
2. ~~**Track 1**~~ — ✅ done (the largest single-bench win).
3. **Track 2** — ~1 day + careful dict-equality-semantics review.
4. **Track 4** — multi-day, in the incremental steps above, memory-model-checks in anger.
5. **Track 5** — opportunistic alongside the others.
6. **Track 6** — scope after the post-Track-1–3 CodSpeed re-profile shows how much
   char counting remains; lever 1 (cached count on `Str`) first, `bytecount` only if
   long-string scans still register.

Rough overall expectation if all land: re_extract ~3.4×→~1.5×, agg_rows ~2.8×→~2×,
fib ~2.3×→~1.6×, str_parse ~2.2×→~1.9×, moving the 21-bench geomean from ~1.5× toward
~1.2–1.3× of CPython.

Also: https://github.com/dtolnay/zmij for Float formatting
