# /// monty
# call-external = true
# ///
import sys
import threading
from threading import Lock, RLock

assert isinstance(Lock(), threading.Lock)
for factory in (Lock, RLock):
    lock = factory()
    acquire = lock.acquire
    release = lock.release
    assert not lock.locked()
    assert acquire(blocking=False) is True
    assert lock.locked()
    assert not factory().locked()
    assert release() is None
    try:
        with lock as acquired:
            assert acquired is True
            assert lock.locked()
            raise ValueError('inside with')
    except ValueError as exc:
        assert str(exc) == 'inside with'
    assert not lock.locked()
    assert acquire(timeout=1) is True
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
    for timeout, error in [(None, TypeError), (float('nan'), ValueError), (float('inf'), OverflowError)]:
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
