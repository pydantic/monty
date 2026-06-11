"""Shared pytest configuration for the pydantic_monty test suite."""

import os
import subprocess
from pathlib import Path


def pytest_configure(config: object) -> None:
    """Points `MontyPool` at the workspace `monty` binary.

    The wheel bundles the CLI binary, but in development the extension module
    is built by maturin without it — so resolve (building if necessary) the
    debug binary from the cargo workspace instead.
    """
    if 'MONTY_BIN' not in os.environ:
        root = Path(__file__).parents[3]
        binary = root / 'target' / 'debug' / ('monty.exe' if os.name == 'nt' else 'monty')
        if not binary.exists():
            subprocess.run(['cargo', 'build', '-p', 'monty-cli'], cwd=root, check=True)
        os.environ['MONTY_BIN'] = str(binary)
