"""Shared pytest configuration for the pydantic_monty test suite."""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any, Callable

import pytest

from pydantic_monty import Monty, MontySession

RunMonty = Callable[..., Any]


def pytest_configure(config: object) -> None:
    """Points the worker pools at the workspace `monty` binary.

    The wheel depends on `pydantic-monty-cli` for the binary, but in
    development the extension module is built by maturin without it — so
    resolve (building if necessary) the debug binary from the cargo workspace
    instead.
    """
    if 'MONTY_BIN' not in os.environ:
        root = Path(__file__).parents[3]
        binary = root / 'target' / 'debug' / ('monty.exe' if os.name == 'nt' else 'monty')
        if not binary.exists():
            subprocess.run(['cargo', 'build', '-p', 'monty-cli'], cwd=root, check=True)
        os.environ['MONTY_BIN'] = str(binary)


@pytest.fixture(scope='session')
def pool() -> Iterator[Monty]:
    """One worker pool shared by the whole test session (workers are reused
    across checkouts, and the pool transparently replaces crashed ones)."""
    with Monty() as p:
        yield p


@pytest.fixture
def session(pool: Monty) -> Iterator[MontySession]:
    """A fresh checked-out session (fresh sandbox state) for one test."""
    with pool.checkout() as s:
        yield s


@pytest.fixture
def monty_run(pool: Monty) -> RunMonty:
    """Runs one snippet in a fresh session and returns its result.

    Checkout-level kwargs (`script_name`, `limits`, `type_check`,
    `type_check_stubs`, `dataclass_registry`) are split out automatically;
    everything else is passed to `feed_run`.
    """

    def run(code: str, **kwargs: Any) -> Any:
        checkout_keys = ('script_name', 'limits', 'type_check', 'type_check_stubs', 'dataclass_registry')
        checkout_kwargs = {k: kwargs.pop(k) for k in checkout_keys if k in kwargs}
        with pool.checkout(**checkout_kwargs) as s:
            return s.feed_run(code, **kwargs)

    return run
