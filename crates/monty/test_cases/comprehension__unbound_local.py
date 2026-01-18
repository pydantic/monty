# xfail=cpython
# Test that comprehension scoping raises NameError when a generator's iter
# references a later generator's loop variable (which is local but not yet assigned)
#
# Note: CPython raises UnboundLocalError here, but Monty raises NameError since we
# don't distinguish between undefined locals and undefined names. This is acceptable
# because the code would fail either way.

z = ['outer']

result = [x for x in [1] for y in z for z in [[2], [3]]]
"""
TRACEBACK:
Traceback (most recent call last):
  File "comprehension__unbound_local.py", line 11, in <module>
    result = [x for x in [1] for y in z for z in [[2], [3]]]
                                      ~
NameError: name 'z' is not defined
"""
