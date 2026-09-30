exec('def f():\n    raise ValueError("boom")\nf()')
"""
TRACEBACK:
Traceback (most recent call last):
  File "builtin__exec_traceback.py", line 1, in <module>
    exec('def f():\n    raise ValueError("boom")\nf()')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
  File "<string>", line 3, in <module>
  File "<string>", line 2, in f
ValueError: boom
"""
