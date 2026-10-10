match x:
    case y:
        pass
    case 1:
        pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__error_capture_unreachable.py", line 2
    case y:
         ~
SyntaxError: name capture 'y' makes remaining patterns unreachable
"""
