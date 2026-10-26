match x:
    case {'a': 1, 'a': 2}:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_duplicate_key.py", line 2
    case {'a': 1, 'a': 2}:
         ~~~~~~~~~~~~~~~~
SyntaxError: mapping pattern checks duplicate key ('a')
"""
