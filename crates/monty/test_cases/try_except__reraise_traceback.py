def fail():
    raise ValueError('original')


def run():
    try:
        fail()
    except ValueError:
        raise


run()
"""
TRACEBACK:
Traceback (most recent call last):
  File "try_except__reraise_traceback.py", line 12, in <module>
    run()
    ~~~~~
  File "try_except__reraise_traceback.py", line 7, in run
    fail()
    ~~~~~~
  File "try_except__reraise_traceback.py", line 2, in fail
    raise ValueError('original')
ValueError: original
"""
