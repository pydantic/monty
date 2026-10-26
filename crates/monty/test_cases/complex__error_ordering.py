def is_less(a, b):
    return a < b


is_less(1 + 1j, 2j)
"""
TRACEBACK:
Traceback (most recent call last):
  File "complex__error_ordering.py", line 5, in <module>
    is_less(1 + 1j, 2j)
    ~~~~~~~~~~~~~~~~~~~
  File "complex__error_ordering.py", line 2, in is_less
    return a < b
           ~~~~~
TypeError: '<' not supported between instances of 'complex' and 'complex'
"""
