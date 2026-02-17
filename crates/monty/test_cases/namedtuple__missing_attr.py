# Test AttributeError message for missing attribute on named tuple
import sys

sys.version_info.foobar
"""
TRACEBACK:
Traceback (most recent call last):
  File "namedtuple__missing_attr.py", line 4, in <module>
    sys.version_info.foobar
AttributeError: 'sys.version_info' object has no attribute 'foobar'
"""
