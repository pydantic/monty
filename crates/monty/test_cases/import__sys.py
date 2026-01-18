# xfail=cpython
# Tests for sys module import

import sys

# === sys.version ===
assert sys.version == '3.14.0 (Monty)', f'version should start with 3.14: {sys.version!r}'

# === sys.version_info ===
# Test index access
assert sys.version_info[0] == 3, 'major version should be 3'
assert sys.version_info[1] == 14, 'minor version should be 14'
assert sys.version_info[2] == 0, 'micro version should be 0'
assert sys.version_info[3] == 'final', 'releaselevel should be final'
assert sys.version_info[4] == 0, 'serial should be 0'

# Test negative indexing
assert sys.version_info[-1] == 0, 'last element (serial) should be 0'
assert sys.version_info[-2] == 'final', 'second-to-last (releaselevel) should be final'
assert sys.version_info[-5] == 3, 'first element via negative index should be 3'

# Test named attribute access
assert sys.version_info.major == 3, 'major attr should be 3'
assert sys.version_info.minor == 14, 'minor attr should be 14'
assert sys.version_info.micro == 0, 'micro attr should be 0'
assert sys.version_info.releaselevel == 'final', 'releaselevel attr should be final'
assert sys.version_info.serial == 0, 'serial attr should be 0'

# Test len
assert len(sys.version_info) == 5, 'version_info should have 5 elements'

# Test tuple indexing works (slice syntax not yet supported)
v = sys.version_info
assert (v[0], v[1]) == (3, 14), 'version should be (3, 14)'
assert v.major == v[0], 'major attr should equal index 0'
assert v.minor == v[1], 'minor attr should equal index 1'

# === sys.platform ===
assert sys.platform == 'monty', f'platform should be monty: {sys.platform!r}'

# === sys.stdout and sys.stderr ===
# These should exist - we test by accessing them (will fail if not present)
stdout = sys.stdout
stderr = sys.stderr
