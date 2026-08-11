"""Resource-limit tests: limits are a `pool.checkout(limits=...)` argument enforced in the worker."""

from __future__ import annotations

import time

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyRuntimeError, ResourceLimits


def test_resource_limits_typed_dict():
    limits = ResourceLimits(
        max_duration_secs=5.0,
        max_memory=1024,
        gc_interval=10,
        max_recursion_depth=500,
    )
    assert limits.get('max_duration_secs') == snapshot(5.0)
    assert limits.get('max_memory') == snapshot(1024)
    assert limits.get('gc_interval') == snapshot(10)
    assert limits.get('max_recursion_depth') == snapshot(500)


def test_resource_limits_repr():
    limits = ResourceLimits(max_duration_secs=1.0)
    assert repr(limits) == snapshot("{'max_duration_secs': 1.0}")


def test_run_with_limits(monty_run: RunMonty):
    assert monty_run('1 + 1', limits={'max_duration_secs': 5.0}) == snapshot(2)


def test_recursion_limit(monty_run: RunMonty):
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(10)
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, limits={'max_recursion_depth': 5})
    assert isinstance(exc_info.value.exception(), RecursionError)


def test_recursion_limit_ok(monty_run: RunMonty):
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
"""
    assert monty_run(code, limits={'max_recursion_depth': 100}) == snapshot(5)


def test_memory_limit(monty_run: RunMonty):
    code = """
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, limits={'max_memory': 100})
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_timeout_limit(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('while True:\n    pass', limits={'max_duration_secs': 0.1})
    inner = exc_info.value.exception()
    assert isinstance(inner, TimeoutError)
    assert exc_info.value.display(format='type-msg').startswith('TimeoutError: time limit exceeded')


def test_session_exhausted_after_resource_error_but_worker_reusable(pool: Monty):
    """A resource error leaves the session exhausted (later feeds keep failing),
    but the worker is reusable once the session exits."""
    with pool.checkout(limits={'max_duration_secs': 0.1}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('while True:\n    pass')
        assert isinstance(exc_info.value.exception(), TimeoutError)
        # the session stays exhausted after a resource error
        with pytest.raises(MontyRuntimeError):
            session.feed_run('1 + 1')
    # a new session reuses the worker without issue
    with pool.checkout() as session:
        assert session.feed_run('1 + 1') == snapshot(2)


def test_limits_with_inputs(monty_run: RunMonty):
    assert monty_run('x * 2', inputs={'x': 21}, limits={'max_duration_secs': 5.0}) == snapshot(42)


def test_limits_wrong_type_raises_error(pool: Monty):
    with pytest.raises(TypeError):
        with pool.checkout(limits={'max_memory': 'not an int'}):  # pyright: ignore[reportArgumentType]
            pass


def test_limits_none_value_allowed(monty_run: RunMonty):
    # None is valid to explicitly disable a limit
    assert monty_run('1 + 1', limits={'max_memory': None}) == snapshot(2)


def test_pow_memory_limit(monty_run: RunMonty):
    """Large pow should fail when memory limit is set."""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('2 ** 10000000', limits={'max_memory': 1_000_000})
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_lshift_memory_limit(monty_run: RunMonty):
    """Large left shift should fail when memory limit is set."""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('1 << 10000000', limits={'max_memory': 1_000_000})
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_mult_memory_limit(monty_run: RunMonty):
    """Large multiplication should fail when memory limit is set."""
    # First create a large number, then try to square it
    code = """
big = 2 ** 4000000
result = big * big
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, limits={'max_memory': 1_000_000})
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_small_operations_within_limit(monty_run: RunMonty):
    """Smaller operations should succeed even with limits."""
    result = monty_run('2 ** 1000', limits={'max_memory': 1_000_000})
    assert result > 0


@pytest.mark.parametrize(
    'code',
    [
        'sum(range(10**18))',
        'list(range(10**18))',
        'sorted(range(10**18))',
        'min(range(10**18))',
        'max(range(10**18))',
    ],
    ids=['sum', 'list', 'sorted', 'min', 'max'],
)
def test_timeout_enforced_in_builtin_loops(monty_run: RunMonty, code: str):
    """Timeout must be enforced inside Rust-side builtin iteration loops.

    Previously, builtins like sum(), sorted(), min(), max() ran Rust-side loops
    entirely within a single bytecode instruction, bypassing the VM's
    per-instruction timeout check.
    """
    start = time.monotonic()
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code, limits={'max_duration_secs': 0.1})
    elapsed = time.monotonic() - start
    assert isinstance(exc_info.value.exception(), TimeoutError)
    # Should terminate promptly - well under 2 seconds
    assert elapsed < 2.0
