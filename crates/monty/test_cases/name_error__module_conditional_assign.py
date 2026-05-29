# Module-scope read of a name whose later assignment didn't execute must raise
# `NameError`, not `UnboundLocalError` — module scope only has `NameError`.


def boom():
    raise ValueError('nope')


try:
    boom()
    foo = 1
except ValueError:
    pass

print(foo)
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__module_conditional_assign.py", line 15, in <module>
    print(foo)
          ~~~
NameError: name 'foo' is not defined
"""
