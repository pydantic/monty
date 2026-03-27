# xfail=cpython
import json

value = []
value.append(value)
json.dumps(value)
"""
TRACEBACK:
Traceback (most recent call last):
  File "json__dumps_circular.py", line 6, in <module>
    json.dumps(value)
    ~~~~~~~~~~~~~~~~~
ValueError: Circular reference detected
"""
