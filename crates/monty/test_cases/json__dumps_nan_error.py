# xfail=cpython
import json

json.dumps(float('nan'), allow_nan=False)
"""
TRACEBACK:
Traceback (most recent call last):
  File "json__dumps_nan_error.py", line 4, in <module>
    json.dumps(float('nan'), allow_nan=False)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: Out of range float values are not JSON compliant: nan
"""
