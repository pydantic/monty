# /// monty
# call-external = true
# ///
def handler_type():
    add_ints(1, 2)
    return ValueError


try:
    1 / 0
except handler_type():
    pass
"""
TRACEBACK:
Traceback (most recent call last):
  File "traceback__unmatched_except_after_resume.py", line 10, in <module>
    1 / 0
    ~~~~~
ZeroDivisionError: division by zero
"""
