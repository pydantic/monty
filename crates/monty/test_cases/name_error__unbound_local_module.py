# Read-before-write at module scope must raise `NameError`, not `UnboundLocalError`.
print(x)
x = 1
print(x)
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__unbound_local_module.py", line 2, in <module>
    print(x)
          ~
NameError: name 'x' is not defined
"""
