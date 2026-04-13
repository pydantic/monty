# call-external
def wrapper():
    raise_error('ValueError', 'from external')


wrapper()
"""
TRACEBACK:
Traceback (most recent call last):
  File "ext_call__exc_in_function.py", line 6, in <module>
    wrapper()
    ~~~~~~~~~
  File "ext_call__exc_in_function.py", line 3, in wrapper
    raise_error('ValueError', 'from external')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: from external
"""
