match x:
    case {0j: a, -0.0: b}:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_zero_key.py", line 2
    case {0j: a, -0.0: b}:
         ~~~~~~~~~~~~~~~~
SyntaxError: mapping pattern checks duplicate key (-0.0)
"""
