# mode: iter
# Test that accessing a non-existent attribute on a dataclass raises AttributeError
point = make_point()
point.z
"""
TRACEBACK:
Traceback (most recent call last):
  File "dataclass__get_missing_attr_error.py", line 4, in <module>
    point.z
AttributeError: 'Point' object has no attribute 'z'
"""
