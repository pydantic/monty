"""Shared pytest configuration for the pydantic_monty test suite."""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any, Callable, Literal, Protocol

import pytest

from pydantic_monty import (
    AbstractOS,
    CollectStreams,
    CollectString,
    Monty,
    MontySession,
    MountDir,
    OsFunction,
    ResourceLimits,
)


class RunMonty(Protocol):
    def __call__(
        self,
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_lookup: dict[str, Any] | None = None,
        print_callback: Callable[[Literal['stdout', 'stderr'], str], None]
        | CollectStreams
        | CollectString
        | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | AbstractOS | None = None,
        skip_type_check: bool = False,
        limits: ResourceLimits | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> Any: ...


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
    """Runs one snippet in a fresh session and returns its result."""

    def run(
        code: str,
        *,
        inputs: dict[str, Any] | None = None,
        external_lookup: dict[str, Any] | None = None,
        print_callback: Callable[[Literal['stdout', 'stderr'], str], None]
        | CollectStreams
        | CollectString
        | None = None,
        mount: MountDir | list[MountDir] | None = None,
        os: Callable[[OsFunction, tuple[Any, ...], dict[str, Any]], Any] | AbstractOS | None = None,
        skip_type_check: bool = False,
        limits: ResourceLimits | None = None,
        dataclass_registry: list[type] | None = None,
    ) -> Any:
        with pool.checkout(limits=limits, dataclass_registry=dataclass_registry) as s:
            return s.feed_run(
                code,
                inputs=inputs,
                external_lookup=external_lookup,
                print_callback=print_callback,
                mount=mount,
                os=os,
                skip_type_check=skip_type_check,
            )

    return run
