# call-external
from pathlib import Path

Path('/nonexistent').read_text()
"""
TRACEBACK:
Traceback (most recent call last):
  File "pathlib__os_read_error.py", line 4, in <module>
    Path('/nonexistent').read_text()
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
FileNotFoundError: [Errno 2] No such file or directory: '/nonexistent'
"""
