# Regression: a function declaring `global X` for a name that never appears at
# module scope used to make the prepare phase allocate a function-local slot but
# tag it `NameScope::Global`. The compiler then emitted a `LoadGlobal slot`
# whose operand was a local index — indexing past the module globals array and
# panicking with `index out of bounds`.


def f(a):
    global ghost
    return ghost


f(0)
"""
TRACEBACK:
Traceback (most recent call last):
  File "global__oob_regression.py", line 13, in <module>
    f(0)
    ~~~~
  File "global__oob_regression.py", line 10, in f
    return ghost
           ~~~~~
NameError: name 'ghost' is not defined
"""
