# call-external
# Test that calling a field value (not a method) raises TypeError
point = make_point()
point.x()
"""
TRACEBACK:
Traceback (most recent call last):
  File "dataclass__call_field_error.py", line 4, in <module>
    point.x()
    ~~~~~~~~~
TypeError: 'int' object is not callable
"""
