# Test that comprehension scoping raises UnboundLocalError when a generator's iter
# references a later generator's loop variable (which is local but not yet assigned)

z = ['outer']

result = [x for x in [1] for y in z for z in [[2], [3]]]
"""
TRACEBACK:
Traceback (most recent call last):
  File "comprehension__unbound_local.py", line 6, in <module>
    result = [x for x in [1] for y in z for z in [[2], [3]]]
                                      ~
UnboundLocalError: cannot access local variable 'z' where it is not associated with a value
"""
