class Number:
    def __round__(self, ndigits):
        raise ValueError('rounding failed')


def rounded(number):
    return 1 + round(number, ['precision'])


rounded(Number())
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__numeric_traceback.py", line 10, in <module>
    rounded(Number())
    ~~~~~~~~~~~~~~~~~
  File "class__numeric_traceback.py", line 7, in rounded
    return 1 + round(number, ['precision'])
               ~~~~~~~~~~~~~~~~~~~~~~~~~~~~
  File "class__numeric_traceback.py", line 3, in __round__
    raise ValueError('rounding failed')
ValueError: rounding failed
"""
