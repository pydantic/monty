# run-async
await 123  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "async__not_awaitable.py", line 2, in <module>
    await 123  # pyright: ignore
    ~~~~~~~~~
TypeError: 'int' object can't be awaited
"""
