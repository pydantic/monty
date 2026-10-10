class P:
    pass


match x:
    case P(x=1, x=2):
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_repeated_attr.py", line 6
    case P(x=1, x=2):
                  ~
SyntaxError: attribute name repeated in class pattern: x
"""
