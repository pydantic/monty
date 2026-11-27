def fail():
    raise ValueError('boom')


def forward():
    try:
        fail()
    except (TypeError, KeyError):
        pass
    except AttributeError:
        raise StopIteration


forward()
"""
TRACEBACK:
Traceback (most recent call last):
  File "traceback__unmatched_except.py", line 14, in <module>
    forward()
    ~~~~~~~~~
  File "traceback__unmatched_except.py", line 7, in forward
    fail()
    ~~~~~~
  File "traceback__unmatched_except.py", line 2, in fail
    raise ValueError('boom')
ValueError: boom
"""
