# Test that bytes.index with start > end doesn't panic but raises ValueError
b'hello'.index(b'e', 5, 2)
"""
TRACEBACK:
Traceback (most recent call last):
  File "bytes__index_start_gt_end.py", line 2, in <module>
    b'hello'.index(b'e', 5, 2)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: subsection not found
"""
