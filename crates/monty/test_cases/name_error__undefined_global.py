# Test that accessing an undefined global name raises NameError
unknown_func()
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__undefined_global.py", line 2, in <module>
    unknown_func()
    ~~~~~~~~~~~~
NameError: name 'unknown_func' is not defined
"""
