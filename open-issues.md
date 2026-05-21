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

## Behaviour divergence from CPython (not yet fixed)

### 4. `+` (update) modes are broken

For `r+`, the first `write()` dispatches `WriteText`/`WriteBytes`, which
truncates the entire file (`fs::write`). CPython `r+` overwrites from the current
position without truncating — silent data loss. `w+`/`r+` `read()` reads the full
on-disk content (no position tracking). Only `r+b` + `read()` is tested.
Consider rejecting `+` modes (like `x`) until position tracking exists.

### 5. No read position state

`read()` always reads the whole file from disk. `f.read(); f.read()` returns the
full content twice; CPython returns `''` the second time.

### 6. Error message mismatches

- `invalid_mode` uses `{mode:?}` → `invalid mode: "z"` (double quotes); CPython
  uses `invalid mode: 'z'`.
- Empty mode → Monty `invalid mode: ''`; CPython →
  `Must have exactly one of create/read/write/append mode and at most one plus`.
- `one_action_mode_error` is lowercased and missing `... and at most one plus`.
- `read`/`write` capability errors raise `OSError`; CPython raises
  `io.UnsupportedOperation` (subclass of `OSError`+`ValueError`). Messages match,
  type does not.

### 7. `py_eq` always returns `Ok(false)`

Even for the same object — `f == f` is `False` in Monty, `True` in CPython.

### 8. `seekable()` hardcoded to `False`

Regular files are seekable (`True`) in CPython.

### 9. Minor

`extract_path_string` rejects `bytes` paths although the error text claims
"expected str, bytes or os.PathLike"; `open(p, 'r', -1)` (positional buffering)
raises "at most 2 args" where CPython accepts it.

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
