match x:
    case _ | 1:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_or_wildcard_unreachable.py", line 2
    case _ | 1:
         ~
SyntaxError: wildcard makes remaining patterns unreachable
"""
