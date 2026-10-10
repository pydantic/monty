match x:
    case {2j: a, 2j: b}:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_complex_key.py", line 2
    case {2j: a, 2j: b}:
         ~~~~~~~~~~~~~~
SyntaxError: mapping pattern checks duplicate key (2j)
"""
