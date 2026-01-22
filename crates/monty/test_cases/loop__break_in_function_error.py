def foo():
    break


foo()
"""
TRACEBACK:
Traceback (most recent call last):
  File "loop__break_in_function_error.py", line 2
    break
    ~~~~~
SyntaxError: 'break' outside loop
"""
