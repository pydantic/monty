# mode: iter
# Test that assigning to a frozen dataclass raises AttributeError
point = make_point()
point.x = 10
"""
TRACEBACK:
Traceback (most recent call last):
  File "dataclass__frozen_set_error.py", line 4, in <module>
    point.x = 10
    ~~~~~~~
AttributeError: 'Point' object attribute 'x' is read-only
"""
