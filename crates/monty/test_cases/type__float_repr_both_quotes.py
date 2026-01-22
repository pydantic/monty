float("it's \"nice\"")
"""
TRACEBACK:
Traceback (most recent call last):
  File "type__float_repr_both_quotes.py", line 1, in <module>
    float("it's \"nice\"")
    ~~~~~~~~~~~~~~~~~~~~~~
ValueError: could not convert string to float: 'it\'s "nice"'
"""
