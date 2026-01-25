# run-async
async def foo():
    return 1


coro = foo()
await coro  # pyright: ignore
await coro  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "async__double_await_coroutine.py", line 8, in <module>
    await coro  # pyright: ignore
    ~~~~~~~~~~
RuntimeError: cannot reuse already awaited coroutine
"""
