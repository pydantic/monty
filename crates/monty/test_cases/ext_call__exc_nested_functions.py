# call-external
def inner():
    raise_error('TypeError', 'nested error')


def middle():
    inner()


def outer():
    middle()


outer()
"""
TRACEBACK:
Traceback (most recent call last):
  File "ext_call__exc_nested_functions.py", line 14, in <module>
    outer()
    ~~~~~~~
  File "ext_call__exc_nested_functions.py", line 11, in outer
    middle()
    ~~~~~~~~
  File "ext_call__exc_nested_functions.py", line 7, in middle
    inner()
    ~~~~~~~
  File "ext_call__exc_nested_functions.py", line 3, in inner
    raise_error('TypeError', 'nested error')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
TypeError: nested error
"""
