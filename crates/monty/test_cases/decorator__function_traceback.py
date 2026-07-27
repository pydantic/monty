# A raising decorator in a stack is pinned to its own `@` line, not to the whole
# `def` statement, so the traceback identifies which decorator failed.
def ok(fn):
    return fn


def explode(fn):
    raise RuntimeError('decorator failed')


@ok
@explode
@ok
def target():
    pass


"""
TRACEBACK:
Traceback (most recent call last):
  File "decorator__function_traceback.py", line 12, in <module>
    @explode
     ~~~~~~~
  File "decorator__function_traceback.py", line 8, in explode
    raise RuntimeError('decorator failed')
RuntimeError: decorator failed
"""
