# Verify that ModuleNotFoundError is raised when an unknown module import is actually executed
# (not guarded by TYPE_CHECKING)

condition = True
if condition:
    import nonexistent_at_runtime

"""
TRACEBACK:
Traceback (most recent call last):
  File "import__runtime_error_when_executed.py", line 6, in <module>
    import nonexistent_at_runtime
ModuleNotFoundError: No module named 'nonexistent_at_runtime'
"""
