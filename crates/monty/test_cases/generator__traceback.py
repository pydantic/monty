def inner():
    yield 1
    raise ValueError('inner failed')


def outer():
    result = yield from inner()
    yield result


for item in outer():
    pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "generator__traceback.py", line 11, in <module>
    for item in outer():
                ~~~~~~~
  File "generator__traceback.py", line 7, in outer
    result = yield from inner()
             ~~~~~~~~~~~~~~~~~~
  File "generator__traceback.py", line 3, in inner
    raise ValueError('inner failed')
ValueError: inner failed
"""
