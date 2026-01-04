import pytest
from inline_snapshot import snapshot

import monty


def test_resource_limits_custom():
    limits = monty.ResourceLimits(
        max_allocations=100,
        max_duration_secs=5.0,
        max_memory=1024,
        gc_interval=10,
        max_recursion_depth=500,
    )
    assert limits.get('max_allocations') == snapshot(100)
    assert limits.get('max_duration_secs') == snapshot(5.0)
    assert limits.get('max_memory') == snapshot(1024)
    assert limits.get('gc_interval') == snapshot(10)
    assert limits.get('max_recursion_depth') == snapshot(500)


def test_resource_limits_repr():
    limits = monty.ResourceLimits(max_duration_secs=1.0)
    assert repr(limits) == snapshot("{'max_duration_secs': 1.0}")


def test_run_with_limits():
    m = monty.Monty('1 + 1')
    limits = monty.ResourceLimits(max_duration_secs=5.0)
    assert m.run(limits=limits) == snapshot(2)


def test_recursion_limit():
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(10)
"""
    m = monty.Monty(code)
    limits = monty.ResourceLimits(max_recursion_depth=5)
    with pytest.raises(RecursionError):
        m.run(limits=limits)


def test_recursion_limit_ok():
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
"""
    m = monty.Monty(code)
    limits = monty.ResourceLimits(max_recursion_depth=100)
    assert m.run(limits=limits) == snapshot(5)


def test_allocation_limit():
    # Note: allocation counting may not trigger on all operations
    # Use a more aggressive allocation pattern
    code = """
result = []
for i in range(10000):
    result.append([i])  # Each append creates a new list
len(result)
"""
    m = monty.Monty(code)
    limits = monty.ResourceLimits(max_allocations=5)
    with pytest.raises(MemoryError):
        m.run(limits=limits)


def test_memory_limit():
    code = """
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    m = monty.Monty(code)
    limits = monty.ResourceLimits(max_memory=100)
    with pytest.raises(MemoryError):
        m.run(limits=limits)


def test_limits_with_inputs():
    m = monty.Monty('x * 2', inputs=['x'])
    limits = monty.ResourceLimits(max_duration_secs=5.0)
    assert m.run(inputs={'x': 21}, limits=limits) == snapshot(42)
