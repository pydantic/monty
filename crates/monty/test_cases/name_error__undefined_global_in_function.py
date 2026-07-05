# Regression: a NameError raised from inside a function body for an undefined
# module-level global must report the actual variable name.
#
# Before the global-slot reverse-map fix, the VM looked up the slot through
# the current frame's function-local names table, which doesn't know about
# module-namespace slots — so the error message was either an unrelated
# function-local name at the same slot index or a `<global N>` placeholder.
def f():
    return undefined_in_function


f()
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__undefined_global_in_function.py", line 12, in <module>
    f()
    ~~~
  File "name_error__undefined_global_in_function.py", line 9, in f
    return undefined_in_function
           ~~~~~~~~~~~~~~~~~~~~~
NameError: name 'undefined_in_function' is not defined
"""
