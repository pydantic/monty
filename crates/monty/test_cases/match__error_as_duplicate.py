match x:
    case [a, *rest] as a:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_as_duplicate.py", line 2
    case [a, *rest] as a:
         ~~~~~~~~~~~~~~~
SyntaxError: multiple assignments to name 'a' in pattern
"""
