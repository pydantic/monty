match x:
    case {1: a, True: b}:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_int_key.py", line 2
    case {1: a, True: b}:
         ~~~~~~~~~~~~~~~
SyntaxError: mapping pattern checks duplicate key (True)
"""
