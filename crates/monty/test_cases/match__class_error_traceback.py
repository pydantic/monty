from dataclasses import dataclass


@dataclass
class Point:
    x: int
    y: int


def locate(p):
    match p:
        case Point(x, y, z):
            return x
    return None


locate(Point(1, 2))
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__class_error_traceback.py", line 17, in <module>
    locate(Point(1, 2))
    ~~~~~~~~~~~~~~~~~~~
  File "match__class_error_traceback.py", line 12, in locate
    case Point(x, y, z):
         ~~~~~~~~~~~~~~
TypeError: Point() accepts 2 positional sub-patterns (3 given)
"""
