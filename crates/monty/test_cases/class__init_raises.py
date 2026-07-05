# An exception raised inside __init__ propagates with a traceback that includes
# the constructor-call frame and the __init__ frame, matching CPython exactly
# (the half-built instance is cleaned up, verified under memory-model-checks).


class Account:
    def __init__(self, balance):
        if balance < 0:
            raise ValueError('balance must be non-negative')
        self.balance = balance


Account(-5)
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__init_raises.py", line 13, in <module>
    Account(-5)
    ~~~~~~~~~~~
  File "class__init_raises.py", line 9, in __init__
    raise ValueError('balance must be non-negative')
ValueError: balance must be non-negative
"""
