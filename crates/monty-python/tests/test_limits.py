import multiprocessing
import os
import signal
import time
from types import FrameType

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


def test_limits_wrong_type_raises_error():
    m = monty.Monty('1 + 1')
    with pytest.raises(TypeError):
        m.run(limits={'max_allocations': 'not an int'})  # pyright: ignore[reportArgumentType]


def test_limits_none_value_allowed():
    m = monty.Monty('1 + 1')
    # None is valid to explicitly disable a limit
    assert m.run(limits={'max_allocations': None}) == snapshot(2)  # pyright: ignore[reportArgumentType]


def test_signal_alarm_custom_error():
    """Test that custom signal handlers work during execution."""
    code = """
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(30)
"""
    m = monty.Monty(code)

    def raise_potato(signum: int, frame: FrameType | None) -> None:
        raise ValueError('potato')

    old_handler = signal.signal(signal.SIGALRM, raise_potato)
    try:
        signal.alarm(1)  # Fire after 1 second
        with pytest.raises(ValueError) as exc_info:
            m.run()
        assert exc_info.value.args[0] == snapshot('potato')
    finally:
        signal.alarm(0)  # Cancel any pending alarm
        signal.signal(signal.SIGALRM, old_handler)


def _send_sigint_after_delay(pid: int, delay: float) -> None:
    """Helper function to send SIGINT to a process after a delay."""
    time.sleep(delay)
    os.kill(pid, signal.SIGINT)


def test_keyboard_interrupt():
    """Test that KeyboardInterrupt is raised when SIGINT is sent during execution."""
    code = """
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(35)
"""
    m = monty.Monty(code)

    # Send SIGINT after a short delay using a separate process
    proc = multiprocessing.Process(target=_send_sigint_after_delay, args=(os.getpid(), 0.05))
    proc.start()
    try:
        raised_keyboard_interrupt = False
        try:
            m.run()
        except KeyboardInterrupt:
            raised_keyboard_interrupt = True

        assert raised_keyboard_interrupt, 'Expected KeyboardInterrupt to be raised'
    finally:
        proc.join()
