# A class annotation is evaluated when `__annotations__` is first read, in a
# frame named `__annotate__` as in CPython.
class Bad:
    a: undefined_thing


print(Bad.__annotations__)
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__annotations_error.py", line 7, in <module>
    print(Bad.__annotations__)
          ~~~~~~~~~~~~~~~~~~~
  File "class__annotations_error.py", line 4, in __annotate__
    a: undefined_thing
       ~~~~~~~~~~~~~~~
NameError: name 'undefined_thing' is not defined
"""
