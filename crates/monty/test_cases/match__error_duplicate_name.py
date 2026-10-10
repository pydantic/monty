match x:
    case [a, a]:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_name.py", line 2
    case [a, a]:
             ~
SyntaxError: multiple assignments to name 'a' in pattern
"""
