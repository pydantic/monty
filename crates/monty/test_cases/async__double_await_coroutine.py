# /// monty
# run-async = true
# ///
async def foo():
    return 1


coro = foo()
await coro  # pyright: ignore
await coro  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "async__double_await_coroutine.py", line 10, in <module>
    await coro  # pyright: ignore
    ~~~~~~~~~~
RuntimeError: cannot reuse already awaited coroutine
"""
