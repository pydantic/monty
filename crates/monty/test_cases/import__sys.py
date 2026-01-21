# Tests for sys module import

import sys

# === sys.version ===
# Check that version is a non-empty string (exact value differs between interpreters)
assert isinstance(sys.version, str), 'version should be a string'
assert len(sys.version) > 0, 'version should be non-empty'

# === sys.version_info ===
# Test index access returns integers for first 3 elements
assert isinstance(sys.version_info[0], int), 'major version should be int'
assert isinstance(sys.version_info[1], int), 'minor version should be int'
assert isinstance(sys.version_info[2], int), 'micro version should be int'
assert isinstance(sys.version_info[3], str), 'releaselevel should be str'
assert isinstance(sys.version_info[4], int), 'serial should be int'

# Test negative indexing
assert sys.version_info[-1] == sys.version_info[4], 'negative index -1 should equal index 4'
assert sys.version_info[-2] == sys.version_info[3], 'negative index -2 should equal index 3'
assert sys.version_info[-5] == sys.version_info[0], 'negative index -5 should equal index 0'

# Test named attribute access matches index access
assert sys.version_info.major == sys.version_info[0], 'major attr should equal index 0'
assert sys.version_info.minor == sys.version_info[1], 'minor attr should equal index 1'
assert sys.version_info.micro == sys.version_info[2], 'micro attr should equal index 2'
assert sys.version_info.releaselevel == sys.version_info[3], 'releaselevel attr should equal index 3'
assert sys.version_info.serial == sys.version_info[4], 'serial attr should equal index 4'

# Test len
assert len(sys.version_info) == 5, 'version_info should have 5 elements'

# Test tuple equality (works after fixing NamedTuple equality)
v = sys.version_info
assert (v[0], v[1]) == (v.major, v.minor), 'tuple of indices should equal tuple of attrs'
assert v.major == v[0], 'major attr should equal index 0'
assert v.minor == v[1], 'minor attr should equal index 1'

# === sys.platform ===
# Check that platform is a non-empty string (exact value differs between interpreters)
assert isinstance(sys.platform, str), 'platform should be a string'
assert len(sys.platform) > 0, 'platform should be non-empty'

# === sys.stdout and sys.stderr ===
# These should exist - we test by accessing them (will fail if not present)
stdout = sys.stdout
stderr = sys.stderr
