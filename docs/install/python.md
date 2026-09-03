# Install for Python

```bash
uv add pydantic-monty
```

Or with pip:

```bash
pip install pydantic-monty
```

Requires Python 3.10 or newer.
The download is about 4.5 MB and there is nothing else to run: no daemon, no image, no API key.

```python
from pydantic_monty import Monty

with Monty() as pool:
    with pool.checkout() as session:
        print(session.feed_run('1 + 2'))
        #> 3
```

The import name is `pydantic_monty`.
Continue with the [Python quickstart](../quickstart/python.md).

## What gets installed

`pydantic-monty` is a metapackage with no code of its own; it pins two distributions:

- [`pydantic-monty-client`](https://pypi.org/project/pydantic-monty-client/), the `pydantic_monty` module you import.
- [`pydantic-monty-runtime`](https://pypi.org/project/pydantic-monty-runtime/), the `monty` binary that the worker
  subprocesses run, shipped the same way `uv` and `ruff` ship theirs.

Installing the wheel places the binary in the environment's scripts directory, so there is no extra setup step.
Install `pydantic-monty-client` alone when the binary comes from somewhere else, such as a base image or a system
package.

### Where the worker binary comes from

`Monty(binary_path=...)` overrides binary resolution.
When it is omitted, the binary is resolved from the `MONTY_BIN` environment variable, then the environment's scripts
directory (where `pydantic-monty-runtime` installs it), then `PATH`.

If you are running untrusted code, pin `binary_path` explicitly rather than relying on `PATH`.

## Command line

The `monty` binary is also a REPL and a file runner:

```console
$ monty -c "print('hello world')"
hello world
```

See [command line](../cli.md) for the flags.

## Other languages

Monty is also packaged for [JavaScript](javascript.md) and [Rust](rust.md), and the commercial
[`monty-server`](docker.md) runs it as a container.
Community bindings, maintained outside this repository:

- **Go**: [gomonty](https://github.com/ewhauser/gomonty/)
- **Dart / Flutter**: [dart_monty](https://github.com/runyaga/dart_monty)
  ([pub.dev](https://pub.dev/packages/dart_monty))
