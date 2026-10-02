# call-external
# The host module has no such attribute, so the import names it.
from tools import nope

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__host_cannot_import.py", line 3, in <module>
    from tools import nope
ImportError: cannot import name 'nope' from 'tools' (unknown location)
"""
