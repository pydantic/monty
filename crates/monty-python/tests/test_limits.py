import pytest
from inline_snapshot import snapshot

import monty
from monty import ResourceLimits


def test_resource_limits_is_typeddict():
    """Verify ResourceLimits is a proper TypedDict."""
    assert isinstance(ResourceLimits, type)
    # TypedDict classes have __annotations__
    assert hasattr(ResourceLimits, '__annotations__')
    assert 'max_allocations' in ResourceLimits.__annotations__
    assert 'max_duration_secs' in ResourceLimits.__annotations__


def test_resource_limits_type_annotation():
    """Verify ResourceLimits can be used as a type annotation."""
    limits: ResourceLimits = {'max_duration_secs': 5.0}
    assert limits['max_duration_secs'] == 5.0


def test_run_with_limits():
    m = monty.Monty('1 + 1')
    assert m.run(limits={'max_duration_secs': 5.0}) == snapshot(2)


def test_run_with_all_limits():
    m = monty.Monty('1 + 1')
    limits: ResourceLimits = {
        'max_allocations': 100,
        'max_duration_secs': 5.0,
        'max_memory': 1024 * 1024,
        'gc_interval': 10,
        'max_recursion_depth': 500,
    }
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
    with pytest.raises(RecursionError):
        m.run(limits={'max_recursion_depth': 5})


def test_recursion_limit_ok():
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
"""
    m = monty.Monty(code)
    assert m.run(limits={'max_recursion_depth': 100}) == snapshot(5)


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
    with pytest.raises(MemoryError):
        m.run(limits={'max_allocations': 5})


def test_memory_limit():
    code = """
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    m = monty.Monty(code)
    with pytest.raises(MemoryError):
        m.run(limits={'max_memory': 100})


def test_limits_with_inputs():
    m = monty.Monty('x * 2', inputs=['x'])
    assert m.run(inputs={'x': 21}, limits={'max_duration_secs': 5.0}) == snapshot(42)
