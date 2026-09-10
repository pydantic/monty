def call(function, second):
    if False:
        first = 1
    return function(first, second)


call(lambda a, b: a + b, 2)
"""
TRACEBACK:
Traceback (most recent call last):
  File "name_error__unbound_local_call_args.py", line 7, in <module>
    call(lambda a, b: a + b, 2)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~
  File "name_error__unbound_local_call_args.py", line 4, in call
    return function(first, second)
                    ~~~~~
UnboundLocalError: cannot access local variable 'first' where it is not associated with a value
"""
