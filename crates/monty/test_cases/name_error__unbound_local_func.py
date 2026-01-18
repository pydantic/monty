# Test that accessing a variable before assignment in a function raises UnboundLocalError
# (In function scope, Python pre-scans for assignments so it knows x is local)
def foo():
    print(x)
    x = 1


foo()
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__unbound_local_func.py", line 8, in <module>
    foo()
    ~~~~~
  File "name_error__unbound_local_func.py", line 4, in foo
    print(x)
          ~
UnboundLocalError: cannot access local variable 'x' where it is not associated with a value
"""
