"""Locates the `monty` CLI binary that worker pools run as subprocesses.

The binary ships in the `pydantic-monty-runtime` wheel (a maturin `bin`-bindings
package, the same pattern `uv` and `ruff` use), which installs it into the
environment's scripts directory.
"""

from __future__ import annotations

import os
import shutil
import sys
import sysconfig
from pathlib import Path

_BIN_NAME = 'monty.exe' if sys.platform == 'win32' else 'monty'


def find_monty_binary(explicit: str | Path | None = None) -> str:
    """Resolves the `monty` binary path used to spawn pool workers.

    Resolution order:

    1. the explicit `binary_path` argument, when given
    2. the `MONTY_BIN` environment variable
    3. the environment's scripts directory (where the `pydantic-monty-runtime`
       wheel installs the binary)
    4. a `monty` executable on `PATH`
    5. a cargo-built binary in the monty workspace, when running from an
       editable/development install of this package
    """
    if explicit is not None:
        return str(explicit)
    env = os.environ.get('MONTY_BIN')
    if env:
        return env
    for scripts_dir in _script_dirs():
        candidate = scripts_dir / _BIN_NAME
        if candidate.is_file():
            return str(candidate)
    found = shutil.which('monty')
    if found:
        return found
    dev = _development_binary()
    if dev is not None:
        return str(dev)
    raise FileNotFoundError(
        'could not locate the `monty` binary required to run sandboxed code; '
        'install it with `pip install pydantic-monty-runtime` (or `make dev-py` in the monty repo), '
        'pass binary_path=..., or set the MONTY_BIN environment variable'
    )


def _script_dirs() -> list[Path]:
    """Scripts directories of the current environment, most specific first."""
    dirs = [Path(sysconfig.get_path('scripts'))]
    # user-scheme installs (`pip install --user`) place scripts elsewhere
    try:
        dirs.append(Path(sysconfig.get_path('scripts', f'{os.name}_user')))
    except KeyError:
        pass
    return dirs


def _development_binary() -> Path | None:
    """A cargo-built `monty` next to an editable install inside the monty repo.

    With `maturin develop`, this file lives at
    `<repo>/crates/monty-python/python/pydantic_monty/_binary.py`, so a
    cargo-built binary is at `<repo>/target/{debug,release}/monty`. Prefer the
    most recently built one so `cargo build` and `cargo build --release`
    both behave as expected.
    """
    repo = Path(__file__).resolve().parents[4]
    if not (repo / 'Cargo.toml').is_file():
        return None
    candidates = [path for profile in ('debug', 'release') if (path := repo / 'target' / profile / _BIN_NAME).is_file()]
    return max(candidates, key=lambda path: path.stat().st_mtime, default=None)
