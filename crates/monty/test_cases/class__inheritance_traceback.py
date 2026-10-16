class Parent:
    x = 1


class Child(Parent):
    pass


c = Child()
c.y
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__inheritance_traceback.py", line 10, in <module>
    c.y
AttributeError: 'Child' object has no attribute 'y'
"""
