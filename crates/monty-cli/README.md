# pydantic-monty-runtime

The `monty` CLI binary, packaged for PyPI in the same way `uv` and `ruff`
package theirs: installing this wheel places the compiled binary in the
environment's scripts directory.

It exists so that [`pydantic-monty`](https://pypi.org/project/pydantic-monty/)
— which runs the Monty sandboxed Python interpreter in crash-isolated worker
subprocesses — can find a `monty` binary without any manual setup. It is
installed automatically as a dependency of `pydantic-monty`; you normally
don't install it directly.

The binary is also a standalone CLI for the
[Monty](https://github.com/pydantic/monty) interpreter:

```console
$ monty -c "print('hello world')"
hello world
```
