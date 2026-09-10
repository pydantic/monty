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
        max_suspensions=20,
    )
    assert limits.get('max_duration_secs') == snapshot(5.0)
    assert limits.get('max_memory') == snapshot(1024)
    assert limits.get('gc_interval') == snapshot(10)
    assert limits.get('max_recursion_depth') == snapshot(500)
    assert limits.get('max_suspensions') == snapshot(20)


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
    """A spent `max_duration_secs` budget is cumulative, so later feeds keep failing,
    but the worker is reusable once the session exits.

    This is specific to the duration limit. A `max_memory` trip is not cumulative, so
    later feeds on the same checkout may succeed — against a heap with no guarantees.
    See `limitations/pool-architecture.md`."""
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


def test_suspension_limit(pool: Monty):
    code = """
n = 0
while True:
    try:
        fetch('x')
    except Exception:
        n += 1
"""

    def fetch(url: str) -> None:
        raise ValueError('refused')

    with pool.checkout(limits={'max_suspensions': 3}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run(code, external_lookup={'fetch': fetch})
        assert isinstance(exc_info.value.exception(), RuntimeError)
        assert exc_info.value.display(format='type-msg') == snapshot('RuntimeError: suspension limit 3 exceeded')
        assert session.feed_run('n') == snapshot(3)
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('fetch("y")', external_lookup={'fetch': fetch})
        assert exc_info.value.display(format='type-msg') == snapshot('RuntimeError: suspension limit 3 exceeded')


def test_suspension_limit_defaults_to_one_thousand(pool: Monty):
    """A checkout with no limits still stops a sandbox looping on host calls."""
    code = """
n = 0
while True:
    fetch('x')
    n += 1
"""

    def fetch(url: str) -> None:
        return None

    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run(code, external_lookup={'fetch': fetch})
        assert exc_info.value.display(format='type-msg') == snapshot('RuntimeError: suspension limit 1000 exceeded')
        assert session.feed_run('n') == snapshot(1000)


def test_limits_with_inputs(monty_run: RunMonty):
    assert monty_run('x * 2', inputs={'x': 21}, limits={'max_duration_secs': 5.0}) == snapshot(42)


def test_limits_wrong_type_raises_error(pool: Monty):
    with pytest.raises(TypeError):
        with pool.checkout(limits={'max_memory': 'not an int'}):  # pyright: ignore[reportArgumentType]
            pass


def test_limits_unknown_key_raises_error(pool: Monty):
    with pytest.raises(ValueError) as exc_info:
        with pool.checkout(limits={'max_memroy': 10_000_000}):  # pyright: ignore[reportArgumentType]
            pass
    assert exc_info.value.args[0] == snapshot(
        "unknown limits key 'max_memroy'; accepted keys are 'max_duration_secs', 'max_memory', "
        "'gc_interval', 'max_recursion_depth', 'max_suspensions'"
    )


def test_limits_non_string_key_raises_error(pool: Monty):
    with pytest.raises(ValueError) as exc_info:
        with pool.checkout(limits={1: 100}):  # pyright: ignore[reportArgumentType]
            pass
    assert exc_info.value.args[0] == snapshot(
        "unknown limits key 1; accepted keys are 'max_duration_secs', 'max_memory', "
        "'gc_interval', 'max_recursion_depth', 'max_suspensions'"
    )


def test_limits_unprintable_key_still_raises_value_error(pool: Monty):
    class BadRepr:
        def __repr__(self) -> str:
            raise RuntimeError('boom')

    with pytest.raises(ValueError) as exc_info:
        with pool.checkout(limits={BadRepr(): 1}):  # pyright: ignore[reportArgumentType]
            pass
    assert exc_info.value.args[0] == snapshot(
        "unknown limits key <unprintable key>; accepted keys are 'max_duration_secs', 'max_memory', "
        "'gc_interval', 'max_recursion_depth', 'max_suspensions'"
    )


def test_limits_str_subclass_key_is_honored(monty_run: RunMonty):
    # A str subclass with a custom __hash__ passes a name check but dodges a
    # dict re-lookup under the plain string's hash; extraction reads the value
    # from the same entry as the key, so the limit must still be enforced.
    class WeirdHashKey(str):
        def __hash__(self) -> int:
            return 0

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('2 ** 10000000', limits={WeirdHashKey('max_memory'): 1_000_000})  # pyright: ignore[reportArgumentType]
    assert isinstance(exc_info.value.exception(), MemoryError)


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
