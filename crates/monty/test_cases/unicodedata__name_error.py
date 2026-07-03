import unicodedata

# U+FFFF is a permanently-unassigned noncharacter, so it has no name and no
# default is supplied.
unicodedata.name('￿')
"""
TRACEBACK:
Traceback (most recent call last):
  File "unicodedata__name_error.py", line 5, in <module>
    unicodedata.name('￿')
    ~~~~~~~~~~~~~~~~~~~~~
ValueError: no such name
"""
