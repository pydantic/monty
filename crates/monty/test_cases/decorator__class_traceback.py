# A failing decorator in a stack must pin its own expression, not the whole
# `class` statement — otherwise every decorator in a stack reports the same
# location and there is no way to tell which one raised.
def bad(cls):
    raise ValueError('boom')


def ok(cls):
    return cls


@ok
@bad
@ok
class C:
    x = 1
    y = 2


"""
TRACEBACK:
Traceback (most recent call last):
  File "decorator__class_traceback.py", line 13, in <module>
    @bad
     ~~~
  File "decorator__class_traceback.py", line 5, in bad
    raise ValueError('boom')
ValueError: boom
"""
