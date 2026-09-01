generator = (missing for _ in [0])
next(generator)
"""
TRACEBACK:
Traceback (most recent call last):
  File "iter__generator_expr_traceback_name.py", line 2, in <module>
    next(generator)
    ~~~~~~~~~~~~~~~
  File "iter__generator_expr_traceback_name.py", line 1, in <genexpr>
    generator = (missing for _ in [0])
                 ~~~~~~~
NameError: name 'missing' is not defined
"""
