# /// monty
# mount-fs = true
# ///
# A name in `__all__` the module lacks raises `AttributeError` at the star import.
(root / 'star_partial.py').write_text("__all__ = ['there', 'missing']\nthere = 1\n")
from star_partial import *

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__star_missing_name.py", line 6, in <module>
    from star_partial import *
AttributeError: module 'star_partial' has no attribute 'missing'
"""
