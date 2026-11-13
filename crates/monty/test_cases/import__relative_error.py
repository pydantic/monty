# The main script has no package: a relative import raises when it runs, not when compiled.
def never_called():
    from . import foo


from .foo import bar

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__relative_error.py", line 6, in <module>
    from .foo import bar
ImportError: attempted relative import with no known parent package
"""
