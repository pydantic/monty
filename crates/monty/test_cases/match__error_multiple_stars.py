match x:
    case [*a, *b]:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_multiple_stars.py", line 2
    case [*a, *b]:
         ~~~~~~~~
SyntaxError: multiple starred names in sequence pattern
"""
