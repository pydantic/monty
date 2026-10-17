# /// monty
# run-async = true
# ///
async def foo():
    return 1


await asyncio.gather(foo(), foo())  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "async__not_imported.py", line 8, in <module>
    await asyncio.gather(foo(), foo())  # pyright: ignore
          ~~~~~~~
NameError: name 'asyncio' is not defined. Did you forget to import 'asyncio'?
"""
