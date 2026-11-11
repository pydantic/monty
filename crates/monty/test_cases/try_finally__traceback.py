def fail():
    return 1 / 0


def run():
    try:
        try:
            fail()
        finally:
            try:
                raise ValueError('handled during cleanup')
            except ValueError:
                pass
            cleanup = True
    finally:
        cleanup = False


try:
    run()
except TypeError:
    pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "try_finally__traceback.py", line 20, in <module>
    run()
    ~~~~~
  File "try_finally__traceback.py", line 8, in run
    fail()
    ~~~~~~
  File "try_finally__traceback.py", line 2, in fail
    return 1 / 0
           ~~~~~
ZeroDivisionError: division by zero
"""
