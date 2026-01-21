from .foo import bar

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__relative_error.py", line 1, in <module>
    from .foo import bar
ImportError: attempted relative import with no known parent package
"""
