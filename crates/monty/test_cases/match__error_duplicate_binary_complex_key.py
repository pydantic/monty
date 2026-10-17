match x:
    case {1 + 2j: a, 1.0 + 2j: b}:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_binary_complex_key.py", line 2
    case {1 + 2j: a, 1.0 + 2j: b}:
         ~~~~~~~~~~~~~~~~~~~~~~~~
SyntaxError: mapping pattern checks duplicate key ((1+2j))
"""
