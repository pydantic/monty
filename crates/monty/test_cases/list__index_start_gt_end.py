# Test that list.index with start > end doesn't panic but raises ValueError
[1, 2, 3].index(1, 5, 2)
"""
TRACEBACK:
Traceback (most recent call last):
  File "list__index_start_gt_end.py", line 2, in <module>
    [1, 2, 3].index(1, 5, 2)
    ~~~~~~~~~~~~~~~~~~~~~~~~
ValueError: list.index(x): x not in list
"""
