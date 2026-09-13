def fail(x):
    raise ValueError('boom')


generator = (fail(x) for x in [1])
list(generator)
"""
TRACEBACK:
Traceback (most recent call last):
  File "iter__generator_expr_traceback_list.py", line 6, in <module>
    list(generator)
    ~~~~~~~~~~~~~~~
  File "iter__generator_expr_traceback_list.py", line 5, in <genexpr>
    generator = (fail(x) for x in [1])
                 ~~~~~~~
  File "iter__generator_expr_traceback_list.py", line 2, in fail
    raise ValueError('boom')
ValueError: boom
"""
