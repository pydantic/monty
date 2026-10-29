class Config:
    @property
    def port(self):
        raise ValueError('no port configured')


port = Config().port
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__property_traceback.py", line 7, in <module>
    port = Config().port
           ~~~~~~~~~~~~~
  File "class__property_traceback.py", line 4, in port
    raise ValueError('no port configured')
ValueError: no port configured
"""
