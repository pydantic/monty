# /// monty
# run-async = true
# ///
# Test that exceptions in async functions produce correct tracebacks


async def raises_error():
    raise ValueError('async error')


await raises_error()  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "async__traceback.py", line 11, in <module>
    await raises_error()  # pyright: ignore
    ~~~~~~~~~~~~~~~~~~~~
  File "async__traceback.py", line 8, in raises_error
    raise ValueError('async error')
ValueError: async error
"""
