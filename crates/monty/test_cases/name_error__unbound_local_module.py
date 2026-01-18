# Test that accessing a variable before assignment at module level raises NameError
# (Unlike function scope, module level doesn't pre-scan for assignments)
print(x)
x = 1
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__unbound_local_module.py", line 3, in <module>
    print(x)
          ~
NameError: name 'x' is not defined
"""
