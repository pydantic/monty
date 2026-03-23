# call-external
# Test that augmented assignment on a frozen dataclass raises FrozenInstanceError
point = make_point()
point.x += 5
"""
TRACEBACK:
Traceback (most recent call last):
  File "dataclass__frozen_augmented_assign_error.py", line 4, in <module>
    point.x += 5
    ~~~~~~~
FrozenInstanceError: cannot assign to field 'x'
"""
