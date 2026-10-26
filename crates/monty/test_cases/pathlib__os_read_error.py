# /// monty
# call-external = true
# skip-cpython-windows = true
# ///
from pathlib import Path

Path('/nonexistent').read_text()
"""
TRACEBACK:
Traceback (most recent call last):
  File "pathlib__os_read_error.py", line 7, in <module>
    Path('/nonexistent').read_text()
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
FileNotFoundError: [Errno 2] No such file or directory: '/nonexistent'
"""
