# xfail=cpython
# Tests for Monty-specific sys module values

import sys

# === sys.version ===
assert sys.version == '3.14.0 (Monty)', f'version should be 3.14.0 (Monty), got {sys.version!r}'

# === sys.version_info exact values ===
assert sys.version_info[0] == 3
assert sys.version_info[1] == 14
assert sys.version_info[2] == 0
assert sys.version_info[3] == 'final'
assert sys.version_info[4] == 0

# === sys.version_info named attributes ===
assert sys.version_info.major == 3
assert sys.version_info.minor == 14
assert sys.version_info.micro == 0
assert sys.version_info.releaselevel == 'final'
assert sys.version_info.serial == 0

# === sys.version_info tuple equality ===
# This works because NamedTuple equality compares only by elements, not type_name
assert sys.version_info == (3, 14, 0, 'final', 0)

# === sys.platform ===
assert sys.platform == 'monty', f'platform should be monty, got {sys.platform!r}'
