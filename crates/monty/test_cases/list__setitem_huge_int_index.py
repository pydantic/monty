# Test using a huge LongInt as a list setitem index (should raise IndexError)
# This covers the code path where a LongInt exceeds i64 range
lst = [1, 2, 3]
huge = 2**100
lst[huge] = 42
"""
TRACEBACK:
Traceback (most recent call last):
  File "list__setitem_huge_int_index.py", line 5, in <module>
    lst[huge] = 42
    ~~~~~~~~~
IndexError: cannot fit 'int' into an index-sized integer
"""
