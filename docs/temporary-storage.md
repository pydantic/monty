# Named temporary files

Use `tempfile.NamedTemporaryFile` to write a uniquely named file and delete it when a `with` block exits.
Pass `delete_on_close=False` for context-exit cleanup.

The default mode is `w+b`, supporting binary reads, writes and seeking.
The modes `w`, `wb`, `a` and `ab` are also supported, including equivalent spellings accepted by `open()`.
Closing the file explicitly keeps it accessible by pathname until context exit.
With `delete=False`, the caller owns deletion and `delete_on_close` has no effect.
There is no automatic deletion when an object becomes unreachable or its worker terminates.

## Supplying storage

Temporary files use the same [filesystem mounts](filesystem.md#mounts) as ordinary files.
No storage is supplied by default.
Use [`MountDir`][pydantic_monty.MountDir] to provide a host directory.
Mount a directory private to one sandbox at `/tmp`, or pass `dir=` to select an existing writable directory.

=== "Python"

    ```python
    import tempfile

    from pydantic_monty import Monty, MountDir

    code = """
    import tempfile
    from pathlib import Path
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as report:
        report.write('total: 42')
        text = Path(report.name).read_text()
    text
    """

    with tempfile.TemporaryDirectory() as host_directory:
        with MountDir(
            host_path=host_directory, virtual_path='/tmp', mode='read-write'
        ) as scratch:
            with Monty() as pool:
                with pool.checkout() as session:
                    print(session.feed_run(code, mount=scratch))
                    #> total: 42
    ```

=== "TypeScript"

    ```ts
    import { mkdtempSync, rmSync } from 'node:fs'
    import { tmpdir } from 'node:os'
    import { join } from 'node:path'

    import { Monty } from '@pydantic/monty'
    import { MountDir } from '@pydantic/monty/node'

    const directory = mkdtempSync(join(tmpdir(), 'monty-'))
    try {
      using scratch = new MountDir({ hostPath: directory, virtualPath: '/tmp', mode: 'read-write' })
      await using pool = await Monty.create()
      await using session = await pool.checkout()
      const code = `
    import tempfile
    from pathlib import Path
    with tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as report:
        report.write('total: 42')
        text = Path(report.name).read_text()
    text
    `
      console.log(await session.feedRun(code, { mount: scratch })) // total: 42
    } finally {
      rmSync(directory, { recursive: true })
    }
    ```

The outer temporary directory belongs to host Python and removes the backing storage on exit.
Inside the sandbox, `.name` is an absolute virtual POSIX path.
Host tools need an explicit mapping from that path to their storage.
Read-only mounts reject creation; overlay mounts keep changes in memory and discard them at the end of the feed.
Use read-write storage when files must survive across feeds, supplying the mount on every feed that needs it.
Concurrent host modification during temporary-file creation or cleanup is unsupported.

## Names and cleanup

`prefix`, `suffix` and `dir` default to `tmp`, an empty suffix and `/tmp` respectively.
A candidate contains eight random lowercase letters, digits or underscores between the prefix and suffix.
Creation reserves the name exclusively and requests permissions `0o600`, subject to storage policy.
It retries occupied names up to 20 times and propagates other errors immediately.
Parent directories must already exist.

Context exit closes and deletes the file even if the body raised an exception or the file was already closed.
Deletion can suspend for host I/O or raise an error; successful cleanup does not suppress the body's exception.
Paused or terminated interpreters cannot run cleanup, so hosts must dispose of abandoned storage.

## Snapshots

Interpreter snapshots preserve the naming stream, open file state and pending filesystem calls.
They do not include filesystem contents.
Retain or save the matching backing storage and reattach it when restoring the interpreter.
Restoration can repeat candidate names, which remain protected by exclusive creation and collision retries.
There is no combined interpreter/filesystem checkpoint API.

See [tempfile limitations](limitations/tempfile.md) for unsupported APIs, entropy behavior and differences from CPython.
