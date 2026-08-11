import unicodedata

unicodedata.category('ab')
"""
TRACEBACK:
Traceback (most recent call last):
  File "unicodedata__bad_arg.py", line 3, in <module>
    unicodedata.category('ab')
    ~~~~~~~~~~~~~~~~~~~~~~~~~~
TypeError: category(): argument must be a unicode character, not a string of length 2
"""
