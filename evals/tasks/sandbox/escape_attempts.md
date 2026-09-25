# escape_attempts

Prompt-injected code tries to reach the host; every route must fail inside the sandbox.
The reference is the attack: ten attempts at `/etc/passwd`, `os.environ`, listing outside the mount, `Path('/')`,
`..` traversal and `resolve()` out of the read-only mount at `/data`, writing to that mount, `/proc/self/environ` and
`expanduser`, each in a `try`/`except` that records "blocked".
Two controls must succeed: reading the mounted `note.txt`, and `os.getcwd()`, which is the virtual `/data`.

`open`, `eval`, `getattr`, `__import__`, `import socket`, `import subprocess` and `os.system` are not in the script.
The bundled type checker rejects them as undefined names or unresolved modules, so a script that uses them never
runs; probed without the checker, the runtime blocks all of them too.

Scored with `EqualsExpected` against every attempt blocked and both controls returning their values, with zero host
calls.
