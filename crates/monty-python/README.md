# pydantic-monty-client

Python client for the Monty sandboxed Python interpreter.

Most users want [`pydantic-monty`](https://pypi.org/project/pydantic-monty/)
instead, which pulls in this package plus
[`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/) and
is documented in full on its PyPI page. Install this one directly only when the
`monty` binary is supplied some other way — a base image, a system package, or a
build of this repository:

```bash
pip install pydantic-monty-client
```

## Locating the worker binary

Execution always happens in a pool of `monty` worker subprocesses: a monty
process can never be made fully crash-proof against memory errors (stack
overflows, allocator aborts) triggered by adversarial input, so crash isolation
is built in. Without `pydantic-monty-runtime` installed, `pydantic_monty` has to
find that binary itself, in this order:

1. the `binary_path=` argument to `Monty(...)` / `AsyncMonty(...)`
2. the `MONTY_BIN` environment variable
3. the environment's scripts directory (where `pydantic-monty-runtime` installs it)
4. a `monty` executable on `PATH`
5. a cargo-built binary in the monty workspace, for editable installs of this repo

If none match, constructing a pool raises `FileNotFoundError`. The binary need not
be the same release as this package, but it must speak a compatible wire protocol
version — the worker rejects an incompatible one when a session is checked out,
reporting the range it serves.

## Usage

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

See the [`pydantic-monty`](https://pypi.org/project/pydantic-monty/) README for
async usage, external functions, snapshots, resource limits, type checking and
observability, and `limitations/pool-architecture.md` in the repository for the
behavioural details of subprocess execution.
