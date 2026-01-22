def f(a):
    pass


f(1, **{"foo'": 2})
"""
TRACEBACK:
Traceback (most recent call last):
  File "function__err_unexpected_kwarg_quote.py", line 5, in <module>
    f(1, **{"foo'": 2})
    ~~~~~~~~~~~~~~~~~~~
TypeError: f() got an unexpected keyword argument 'foo''
"""
