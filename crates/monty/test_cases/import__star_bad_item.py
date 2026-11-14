# /// monty
# mount-fs = true
# ///
# `__all__` entries must be strings.
(root / 'star_bad_all.py').write_text("__all__ = ['ok', 1]\nok = 1\n")
from star_bad_all import *

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__star_bad_item.py", line 6, in <module>
    from star_bad_all import *
TypeError: Item in star_bad_all.__all__ must be str, not int
"""
