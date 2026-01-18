import sys

v = sys.version_info
assert (v[0], v[1]) == (3, 14), f'Expected Python 3.14, got ({v[0]}, {v[1]})'
