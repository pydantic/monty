def outer():
    return missing_func()


outer()
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__undefined_call_in_function.py", line 5, in <module>
    outer()
    ~~~~~~~
  File "name_error__undefined_call_in_function.py", line 2, in outer
    return missing_func()
           ~~~~~~~~~~~~
NameError: name 'missing_func' is not defined
"""
