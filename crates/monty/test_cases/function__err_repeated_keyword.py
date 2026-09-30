# A repeated keyword argument is a compile-time SyntaxError, as in CPython
# (ruff's parser no longer reports it, so Monty checks during conversion).
print(x=1, x=2)

"""
TRACEBACK:
Traceback (most recent call last):
  File "function__err_repeated_keyword.py", line 3
    print(x=1, x=2)
               ~~~
SyntaxError: keyword argument repeated: x
"""
