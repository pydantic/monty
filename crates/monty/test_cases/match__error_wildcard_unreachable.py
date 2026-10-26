match x:
    case _:
        pass
    case 1:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_wildcard_unreachable.py", line 2
    case _:
         ~
SyntaxError: wildcard makes remaining patterns unreachable
"""
