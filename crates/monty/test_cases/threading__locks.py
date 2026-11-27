# /// monty
# call-external = true
# ///
import sys
import threading
from threading import Lock, RLock


class IndexedTimeout:
    def __index__(self):
        return 0

    def __float__(self):
        raise AssertionError('acquire must use __index__')


class FloatOnlyTimeout:
    def __float__(self):
        return 0.25


assert type(Lock()) is threading.Lock
if sys.platform == 'monty':
    assert type(RLock()) is threading.RLock
    assert isinstance(RLock(), RLock)
    assert not isinstance(Lock(), RLock)
    assert not isinstance(RLock(), Lock)
for factory in (Lock, RLock):
    lock = factory()
    other = factory()
    name = 'lock' if factory is Lock else 'RLock'
    assert type(lock).__name__ == name
    assert lock != other
    labels = {lock: 'held', other: 'other'}
    try:
        len(lock)
        assert False, 'locks have no length'
    except TypeError as exc:
        assert str(exc) == f"object of type '_thread.{name}' has no len()"
    try:
        lock.clear([])
        assert False, 'lock accepted an unknown method'
    except AttributeError as exc:
        assert str(exc) == f"'_thread.{name}' object has no attribute 'clear'"

    acquire = lock.acquire
    release = lock.release
    assert not lock.locked()
    assert acquire(blocking=False) is True
    assert lock.locked()
    assert not other.locked()
    assert labels[lock] == 'held' and labels[other] == 'other'
    assert release() is None
    try:
        with lock as acquired:
            assert acquired is True
            assert lock.locked()
            raise ValueError('inside with')
    except ValueError as exc:
        assert str(exc) == 'inside with'
    assert not lock.locked()
    assert acquire(timeout=IndexedTimeout()) is True
    release()
    assert acquire(timeout=0.25) is True
    release()

    try:
        acquire(False, 0)
        assert False, 'nonblocking acquire with timeout'
    except ValueError as exc:
        assert str(exc) == "can't specify a timeout for a non-blocking call"
    try:
        acquire(timeout=-2)
        assert False, 'negative timeout'
    except ValueError as exc:
        assert str(exc) == 'timeout value must be a non-negative number'
    try:
        acquire(timeout=FloatOnlyTimeout())
        assert False, 'acquire accepted a __float__-only object'
    except TypeError as exc:
        assert str(exc) == "'FloatOnlyTimeout' object cannot be interpreted as an integer or float"
    for timeout, error in [
        (None, TypeError),
        (10**100, OverflowError),
        (float('nan'), ValueError),
        (float('inf'), OverflowError),
    ]:
        try:
            acquire(timeout=timeout)
            assert False, 'invalid timeout'
        except error:
            pass
        assert not lock.locked()
    try:
        release()
        assert False, 'unbalanced release'
    except RuntimeError as exc:
        assert str(exc) == ('release unlocked lock' if factory is Lock else 'cannot release un-acquired lock')

lock = Lock()
with lock:
    assert lock.acquire(blocking=False) is False
    assert lock.acquire(timeout=0.001) is False
    if sys.platform == 'monty':
        try:
            with lock:
                assert False, 'entered a locked critical section'
        except RuntimeError as exc:
            assert str(exc) == 'cannot wait for a locked lock in Monty'
    assert lock.locked()
assert not lock.locked()

lock = RLock()
with lock:
    with lock:
        assert add_ints(1, 2) == 3
        assert lock.acquire(False) is True
        lock.release()
    assert lock.locked()
assert not lock.locked()
