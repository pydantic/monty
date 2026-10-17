class Protocol:
    def __call__(self, key):
        return self[key]

    def __getitem__(self, key):
        return self + key

    def __add__(self, other):
        raise ValueError('protocol failed')


Protocol()(['key'])
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__protocol_traceback.py", line 12, in <module>
    Protocol()(['key'])
    ~~~~~~~~~~~~~~~~~~~
  File "class__protocol_traceback.py", line 3, in __call__
    return self[key]
           ~~~~~~~~~
  File "class__protocol_traceback.py", line 6, in __getitem__
    return self + key
           ~~~~~~~~~~
  File "class__protocol_traceback.py", line 9, in __add__
    raise ValueError('protocol failed')
ValueError: protocol failed
"""
