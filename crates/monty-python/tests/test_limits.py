import multiprocessing
import os
import signal
import threading
import time
from types import FrameType

import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_resource_limits_custom():
    limits = pydantic_monty.ResourceLimits(
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
    limits = pydantic_monty.ResourceLimits(max_duration_secs=1.0)
    assert repr(limits) == snapshot("{'max_duration_secs': 1.0}")


def test_run_with_limits():
    m = pydantic_monty.Monty('1 + 1')
    limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0)
    assert m.run(limits=limits) == snapshot(2)


def test_recursion_limit():
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(10)
"""
    m = pydantic_monty.Monty(code)
    limits = pydantic_monty.ResourceLimits(max_recursion_depth=5)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(limits=limits)
    assert isinstance(exc_info.value.exception(), RecursionError)


def test_recursion_limit_ok():
    code = """
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
"""
    m = pydantic_monty.Monty(code)
    limits = pydantic_monty.ResourceLimits(max_recursion_depth=100)
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
    m = pydantic_monty.Monty(code)
    limits = pydantic_monty.ResourceLimits(max_allocations=5)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(limits=limits)
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_memory_limit():
    code = """
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    m = pydantic_monty.Monty(code)
    limits = pydantic_monty.ResourceLimits(max_memory=100)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(limits=limits)
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_limits_with_inputs():
    m = pydantic_monty.Monty('x * 2', inputs=['x'])
    limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0)
    assert m.run(inputs={'x': 21}, limits=limits) == snapshot(42)


def test_limits_wrong_type_raises_error():
    m = pydantic_monty.Monty('1 + 1')
    with pytest.raises(TypeError):
        m.run(limits={'max_allocations': 'not an int'})  # pyright: ignore[reportArgumentType]


def test_limits_none_value_allowed():
    m = pydantic_monty.Monty('1 + 1')
    # None is valid to explicitly disable a limit
    assert m.run(limits={'max_allocations': None}) == snapshot(2)  # pyright: ignore[reportArgumentType]


def test_signal_alarm_custom_error():
    """Test that custom signal handlers work during execution.

    The idea here is we run another thread which sends a signal to the current process after a delay
    then set up a signal handler to catch that signal and raise a custom exception.

    So while monty is running, we have to run the code to catch the signal, and propagate that exception.
    """
    code = """
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(35)
"""
    m = pydantic_monty.Monty(code)

    def send_signal():
        time.sleep(0.1)
        os.kill(os.getpid(), signal.SIGINT)

    def raise_potato(signum: int, frame: FrameType | None) -> None:
        raise ValueError('potato')

    thread = threading.Thread(target=send_signal)
    thread.start()
    old_handler = signal.signal(signal.SIGINT, raise_potato)
    try:
        with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
            m.run()
        inner = exc_info.value.exception()
        assert isinstance(inner, ValueError)
        assert inner.args[0] == snapshot('potato')
    finally:
        thread.join()
        signal.signal(signal.SIGINT, old_handler)


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
    m = pydantic_monty.Monty(code)

    # Send SIGINT after a short delay using a separate process
    proc = multiprocessing.Process(target=_send_sigint_after_delay, args=(os.getpid(), 0.05))
    proc.start()
    try:
        raised_keyboard_interrupt = False
        try:
            m.run()
        except pydantic_monty.MontyRuntimeError as e:
            if isinstance(e.exception(), KeyboardInterrupt):
                raised_keyboard_interrupt = True

        assert raised_keyboard_interrupt, 'Expected KeyboardInterrupt to be raised'
    finally:
        proc.join()


def test_pow_memory_limit():
    """Large pow should fail when memory limit is set."""
    m = pydantic_monty.Monty('2 ** 10000000')
    limits = pydantic_monty.ResourceLimits(max_memory=1_000_000)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(limits=limits)
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_lshift_memory_limit():
    """Large left shift should fail when memory limit is set."""
    m = pydantic_monty.Monty('1 << 10000000')
    limits = pydantic_monty.ResourceLimits(max_memory=1_000_000)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(limits=limits)
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_mult_memory_limit():
    """Large multiplication should fail when memory limit is set."""
    # First create a large number, then try to square it
    code = """
big = 2 ** 4000000
result = big * big
"""
    m = pydantic_monty.Monty(code)
    limits = pydantic_monty.ResourceLimits(max_memory=1_000_000)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(limits=limits)
    assert isinstance(exc_info.value.exception(), MemoryError)


def test_small_operations_within_limit():
    """Smaller operations should succeed even with limits."""
    m = pydantic_monty.Monty('2 ** 1000')
    limits = pydantic_monty.ResourceLimits(max_memory=1_000_000)
    result = m.run(limits=limits)
    assert result > 0
