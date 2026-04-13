# call-external
# Test that setting a non-field attribute on frozen dataclass raises error
point = make_point()
point.z = 42
"""
TRACEBACK:
Traceback (most recent call last):
  File "attr__set_frozen_nonfield.py", line 4, in <module>
    point.z = 42
    ~~~~~~~
FrozenInstanceError: cannot assign to field 'z'
"""
