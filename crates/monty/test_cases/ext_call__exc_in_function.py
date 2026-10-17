# /// monty
# call-external = true
# ///
def wrapper():
    raise_error('ValueError', 'from external')


wrapper()
"""
TRACEBACK:
Traceback (most recent call last):
  File "ext_call__exc_in_function.py", line 8, in <module>
    wrapper()
    ~~~~~~~~~
  File "ext_call__exc_in_function.py", line 5, in wrapper
    raise_error('ValueError', 'from external')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: from external
"""
