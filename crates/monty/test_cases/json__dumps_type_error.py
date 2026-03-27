# xfail=cpython
import json

json.dumps({1})
"""
TRACEBACK:
Traceback (most recent call last):
  File "json__dumps_type_error.py", line 4, in <module>
    json.dumps({1})
    ~~~~~~~~~~~~~~~
TypeError: Object of type set is not JSON serializable
"""
