# threading

Monty provides `threading.Lock` and `threading.RLock` for synchronous code.
The rest of the module, including `Thread`, `Condition`, `Event` and `Semaphore`, is unavailable.

- Both constructors are classes in Monty, so `isinstance(lock, threading.RLock)` works.
    CPython 3.14 exposes `threading.RLock` as a factory function.
- Acquiring a held `Lock` with the default blocking behavior raises `RuntimeError` immediately, including on entry
    to a nested `with lock:` block.
    A finite timeout returns `False` immediately because no other thread can release the lock during the wait.
- All code in a session runs on the same thread, including async tasks.
    An `RLock` treats acquisitions from different tasks as reentrant.
- The legacy `acquire_lock`, `release_lock` and `locked_lock` aliases and private lock methods are unavailable.
    Read methods from the instance, as in `lock.release`. Access through the class, as in `Lock.release`, raises
    `AttributeError`.
    Direct dunder calls use the standard Monty context manager signatures. See [with.md](with.md).
- Lock representations omit the acquisition state and thread owner.
