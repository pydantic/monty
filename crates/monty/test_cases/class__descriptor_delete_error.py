class Record:
    pass


record = Record()
del record.missing

"""
TRACEBACK:
Traceback (most recent call last):
  File "class__descriptor_delete_error.py", line 6, in <module>
    del record.missing
        ~~~~~~~~~~~~~~
AttributeError: 'Record' object has no attribute 'missing'
"""
