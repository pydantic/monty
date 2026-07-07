class Foo:
    def __init__(self):
        return 42


Foo()
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__init_return.py", line 6, in <module>
    Foo()
    ~~~~~
TypeError: __init__() should return None, not 'int'
"""
