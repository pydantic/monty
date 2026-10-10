match x:
    case [a] | [b]:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_different_names.py", line 2
    case [a] | [b]:
         ~~~~~~~~~
SyntaxError: alternative patterns bind different names
"""
