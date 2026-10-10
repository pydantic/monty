# call-external
# run-async


def g():
    raise ValueError('original')


async def f():
    try:
        g()
    except ValueError:
        assert await async_call(42) == 42
        raise


await f()  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "traceback__suspended_reraise.py", line 17, in <module>
    await f()  # pyright: ignore
    ~~~~~~~~~
  File "traceback__suspended_reraise.py", line 11, in f
    g()
    ~~~
  File "traceback__suspended_reraise.py", line 6, in g
    raise ValueError('original')
ValueError: original
"""
