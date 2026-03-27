# xfail=cpython
import json

json.loads('{]')
"""
TRACEBACK:
Traceback (most recent call last):
  File "json__loads_error.py", line 4, in <module>
    json.loads('{]')
    ~~~~~~~~~~~~~~~~
json.JSONDecodeError: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)
"""
