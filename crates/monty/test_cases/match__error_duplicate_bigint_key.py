match x:
    case {9223372036854775808: a, 9223372036854775808.0: b}:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_bigint_key.py", line 2
    case {9223372036854775808: a, 9223372036854775808.0: b}:
         ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
SyntaxError: mapping pattern checks duplicate key (9.223372036854776e+18)
"""
