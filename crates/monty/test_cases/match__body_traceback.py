def handle(event):
    match event:
        case {'type': 'click', 'pos': [x, y]} if x > 0:
            return x + y
        case {'type': 'key', 'code': code}:
            raise ValueError(f'unhandled key {code}')
    return None


handle({'type': 'click', 'pos': [1, 2]})
handle({'type': 'key', 'code': 'q'})
"""
TRACEBACK:
Traceback (most recent call last):
  File "match__body_traceback.py", line 11, in <module>
    handle({'type': 'key', 'code': 'q'})
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
  File "match__body_traceback.py", line 6, in handle
    raise ValueError(f'unhandled key {code}')
ValueError: unhandled key q
"""
