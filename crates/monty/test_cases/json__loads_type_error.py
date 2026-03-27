# xfail=cpython
import json

json.loads(1)
"""
TRACEBACK:
Traceback (most recent call last):
  File "json__loads_type_error.py", line 4, in <module>
    json.loads(1)
    ~~~~~~~~~~~~~
TypeError: the JSON object must be str, bytes or bytearray, not int
"""
