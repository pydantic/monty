# `open()` Builtin — Review Findings

Reviewed the single commit `add support for "open()"` (branch `open` vs merge-base `4c333a43`).

Overall the design (path+mode wrapper, no live host FD, each `read`/`write` is a
one-shot `OsFunction`) is sound and preserves the sandbox boundary correctly. The
new `AppendText`/`AppendBytes` go through `resolve_path`/`ResolveMode::Creation`
exactly like writes, no FD is held across calls, and `OpenFile` holds no heap ids
so snapshotting is safe. No security regressions found.

## Bugs

### 1. Refcount leak in `parse_open_args` — `validate_ignored_open_kwarg` error path — DONE

`builtins/open.rs`. Every other error branch in the kwargs loop explicitly drops
`file` and `mode` before returning. The `buffering | encoding | errors | newline`
arm did not — on the `?` failure (e.g. `open(p, encoding=123)`), `file` (a `Path`
Ref) and `mode` were leaked.

Fixed: capture the validation result, then drop `file`/`mode` before propagating.
Regression test added to `open__fs.py` (`open(root / 'hello.txt', encoding=123)`),
verified to fail under `memory-model-checks` before the fix and pass after.

### 2. Refcount leak in `OpenFile::write` — closed-file path — DONE

`types/file.rs`. The `not writable` and `validate_write_data` paths dropped
`data`, but `file.ensure_open()?` did not — `f.close(); f.write(<heap value>)`
leaked the argument's heap refs.

Fixed: the closed-file check now happens before `get_mut`, dropping `data` on
error. Regression test added to `open__fs.py` (closed-file write of a
heap-allocated string), verified to fail under `memory-model-checks` before the
fix and pass after.

### 3. Deferred truncation / `open(path, 'w')` semantics — DONE

`open()` performs the open-time effect immediately, matching CPython
(truncate/create on open, existence check for read modes), via a dedicated
`OsFunction::Open` and a new `MontyObject::FileHandle` boundary type.

What was implemented:

- **`OsFunction::Open` + `MontyObject::FileHandle`.** `builtin_open` allocates
  no heap object: it validates args, allocates a unique file id, and returns
  `CallResult::OsCall(OsFunction::Open, [path, mode, id])`. The host performs
  the open-time effect and returns a `MontyObject::FileHandle`; the generic
  resume path converts it to the `OpenFile` heap wrapper. `open()` therefore
  needs **no** special resume handling — the OS result genuinely *is* the
  value `open()` evaluates to.
- **`FileMode`** (with `FileKind`/`FileAccess`/`OpenAction`) is now a public
  type in `object.rs`, carried structured by `FileHandle`. `OpenFile` stores a
  `FileMode` plus `position`/`id`. Replaced the four mode bools.
- **Host `Open` handler** (`MountTable` direct + overlay backends, and the
  datatest VFS): `w`/`w+` truncate-or-create, `a`/`a+` create-preserving,
  `r`/`r+` existence-check (raising `FileNotFoundError`/`IsADirectoryError`).
  The read-only-mount gate (`FsRequest::is_write`) is mode-aware for `Open`.
- **`Scheduler::pending_open_result` and all its plumbing are deleted** —
  no substitution slot, no `resume`/`resume_with_exception`/`cleanup`
  special-casing. `read()`/`write()` pass the file object itself; the boundary
  converts `OpenFile` ⇄ `FileHandle`.
- Truncating wrappers start in `WriteState::Written` so the first user
  `write()` appends to the freshly-emptied file instead of truncating again.

This also fixed the leftover read-mode-existence divergence: `open(missing,
'r')` now raises `FileNotFoundError` and `open(<dir>, 'r')` raises
`IsADirectoryError` at open time, matching CPython.

Verified: new assertions in `open__fs.py` (open-time truncation/creation,
append preservation, binary, and the `r`-mode existence errors) pass on **both**
Monty and CPython under `memory-model-checks`; new `fs_security` tests cover
`open()` path-traversal and symlink-escape rejection. Full suite green: 948
datatest cases + all `monty` integration tests (`fs`, `fs_security`,
`os_tests`, `repl`/resume), 1044 Python + 344 JS binding tests, `lint-rs`,
`lint-py`.

## Behaviour divergence from CPython

### 4. `+` (update) modes — DONE

Rejected at parse time with `ValueError: update modes ('+') are not yet
supported`. Implementing `+` correctly requires read-position tracking that
the wrapper does not yet have; honoring them naively would truncate on the
first write to an `r+` file. The `FileMode::{ReadUpdate, WriteUpdate,
AppendUpdate}` variants stay in the enum as reserved future scope; `FromStr`
no longer constructs them.

### 5. No read position state — DONE

`OpenFile` now carries a `ReadState { Fresh, Consumed }` flag. The first
`read()` dispatches a full-file OS call and flips the flag to `Consumed`;
subsequent `read()` calls short-circuit to an empty `str`/`bytes` value
without round-tripping to the host, matching CPython's sequential-EOF
behavior.

### 6. Error message mismatches — DONE

- Unknown mode character: now `invalid mode: 'z'` (CPython exact).
- Empty mode: now `Must have exactly one of create/read/write/append mode and
  at most one plus` (CPython exact).
- Duplicate `r`/`w`/`a` keeps the short lowercase
  `must have exactly one of create/read/write/append mode` — that turned out
  to be what CPython actually emits in this case; the doc's original
  recommendation to use the long form here was incorrect.
- `read`/`write` capability errors now raise `io.UnsupportedOperation`
  (new `ExcType::UnsupportedOperation`, slotted under `OSError` in
  `is_subclass_of`). Mapped to/from real `io.UnsupportedOperation` in
  `pydantic_monty`. Single-inheritance limitation: `except ValueError:` will
  not catch it.

### 7. `py_eq` for files — VERIFIED (no fix needed)

`Value::py_eq` already short-circuits `Value::Ref(id) == Value::Ref(id)` to
`Ok(true)` before delegating to the type-specific `HeapRead::py_eq`. So
`f == f` already returns `True` (same heap id) and two distinct file handles
to the same path return `False` (matching CPython). Confirmed by an
assertion added to `open__fs.py`.

### 8. `seekable()` — DONE

Now returns `True`. `seek()` itself is still unimplemented; calling it
raises `AttributeError` separately.

### 9. Minor — DONE

- `extract_path_string` accepts `bytes` paths (UTF-8 decoded; invalid UTF-8
  raises `UnicodeDecodeError`). Outer/inner duplicate `_ =>` error arms
  collapsed into a single `Option`-based fallthrough; same shape applied to
  `extract_mode_string`.
- `parse_open_args` accepts up to 8 positional args (CPython parity).
  Positional indices 3–8 are mapped to their kwarg names
  (`buffering`/`encoding`/`errors`/`newline`/`closefd`/`opener`) and routed
  through the existing `validate_ignored_open_kwarg`. `closefd` and
  `opener` are also accepted as kwargs.

## Usability gap (pre-existing, not a regression)

`with` statements are unsupported language-wide (`parse.rs:412`), and the file
object exposes no `__enter__`/`__exit__`/iteration/`readline`/`read(size)`. The
idiomatic `with open(...) as f:` cannot be used at all. Worth tracking as a
follow-up.

## Performance

### 10. Overlay append is O(n²)

`existing_file_bytes` clones the entire current content, extends, then re-inserts
a new `OverlayEntry::File`. A loop of N appends copies the whole file each time.
When the entry is already `OverlayEntry::File`, take ownership of the existing
`Vec` and `extend_from_slice` in place.

### 11. Minor

`read()`/`write()` clone the path `String` and `allocate_string` a fresh heap
string on every call.

## Cleanup

- `extract_path_string` / `extract_mode_string`: duplicated error arms; collapse
  via a shared fallthrough/helper.
- `OpenFile::write`: replace manual `data.drop_with_heap(vm)` calls with a single
  `HeapGuard`/`defer_drop!`.
- Datatest inconsistency: `OsFunction::WriteText` in `monty-datatest/src/main.rs`
  returns byte count, while the new `AppendText` (and real `write_text_fs`)
  return char count. Align `WriteText` mock to char count.
