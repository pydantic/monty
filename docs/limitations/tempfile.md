# `tempfile` module

Monty implements `NamedTemporaryFile`, `gettempdir` and `gettempprefix`.
See [temporary storage](../temporary-storage.md) for host configuration and examples.

## NamedTemporaryFile

The default mode is `w+b`; `w`, `wb`, `a` and `ab` are also supported.
Equivalent spellings follow `open()` normalization.
Read-only modes and update modes other than binary `w+b` are outside this implementation.
Other file options follow the [open() restrictions](open.md#open-arguments).

With `delete=True`, pass `delete_on_close=False` explicitly.
CPython defaults to deleting on close; Monty rejects that behavior rather than changing its meaning.
Explicit `.close()` keeps the path available until context exit, which closes and unlinks the file.
With `delete=False`, context exit only closes the file and the caller owns deletion.

Deletion on close is deferred to keep this implementation limited to context-exit cleanup.
Ordinary `.close()` only updates interpreter state; deletion would add a host call that can suspend or fail.
Supporting it requires coordinating explicit close with context exit and retaining the closed state if deletion fails.
It does not require native descriptors or update modes.

Cleanup errors propagate and leave the file closed.
A context-exit deletion is attempted once, including if the file was already explicitly closed.
An exception raised by the body is not suppressed by successful cleanup.
There is no cleanup on garbage collection, interpreter shutdown or worker termination.
The host must dispose of abandoned storage.

The returned object uses Monty's ordinary file type, rather than CPython's separate temporary-file wrapper.
There is no `.file` attribute or `_TemporaryFileWrapper` type.
Its `.name` is an absolute virtual path, not a native host pathname.
There is no native descriptor and `fileno()` remains unsupported.

## Paths and storage

`gettempdir()` always returns `/tmp`, and `gettempprefix()` returns `tmp`.
Importing the module or calling these functions does not access storage or initialize random state.
Monty does not inspect environment variables, probe directories or offer a writable `tempdir` setting.
Use `dir=` with a string or `pathlib.Path` to choose a different parent.
Bytes paths are rejected, as are prefixes and suffixes containing `/` or null bytes.
Backslashes are ordinary filename characters under Monty's POSIX path rules.

Storage must be provided through the ordinary mounts or an OS callback.
There is no automatic fallback and missing parent directories are not created.
Temporary storage must be private to one sandbox; concurrent host modification during creation or cleanup is unsupported.
Host tools must finish their filesystem operations before the sandbox resumes those operations.

Names contain eight lowercase letters, digits or underscores.
Creation reserves each candidate exclusively with requested permissions `0o600`, subject to the backing storage's policy.
Only `FileExistsError` triggers a retry; after 20 occupied candidates, creation raises `FileExistsError`.
Other storage errors propagate immediately.
Mount limits apply as for ordinary file operations.

## Random state and snapshots

Temporary naming uses a private MT19937 stream initialized lazily according to `random_start`.
A session seed derives a separate stream without consuming module-level or instance-initialization draws.
`system` and `call_host` obtain 2,496 bytes of entropy through the existing random initialization machinery.
Entropy errors and malformed host replies follow the same rules as `random` initialization.
Calling `random.seed()` does not reset temporary naming.
Exact candidate sequences are not a compatibility guarantee and are not cryptographic secrets.

Random state, pending creation and pending cleanup survive interpreter snapshots.
Older snapshots initialize the private generator on first use under their restored entropy policy.
Restoring a snapshot can repeat candidate names; exclusive creation and collision retries still apply.
Snapshots do not include mounted files: retain the matching storage and reattach it on restoration.
Read-write mounts retain files across feeds, while overlay writes are discarded when a feed ends.

## Unsupported APIs

`TemporaryDirectory`, `mkdtemp`, `TemporaryFile`, `mkstemp`, `mktemp`, `SpooledTemporaryFile`,
`gettempdirb` and `gettempprefixb` are not implemented.
