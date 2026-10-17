# /// monty
# call-external = true
# ///
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
  File "ext_call__exc_nested_functions.py", line 16, in <module>
    outer()
    ~~~~~~~
  File "ext_call__exc_nested_functions.py", line 13, in outer
    middle()
    ~~~~~~~~
  File "ext_call__exc_nested_functions.py", line 9, in middle
    inner()
    ~~~~~~~
  File "ext_call__exc_nested_functions.py", line 5, in inner
    raise_error('TypeError', 'nested error')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
TypeError: nested error
"""
