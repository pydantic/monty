# call-external
# The host answers `__import__('nope')` with not-found.
import tools
import nope

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__host_not_found.py", line 4, in <module>
    import nope
ModuleNotFoundError: No module named 'nope'
"""
