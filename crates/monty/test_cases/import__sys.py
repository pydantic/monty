# Tests for sys module import

import sys

# === sys.version ===
# Check that version is a non-empty string (exact value differs between interpreters)
assert isinstance(sys.version, str)
assert len(sys.version) > 0

# === sys.version_info ===
# Test index access returns integers for first 3 elements
assert isinstance(sys.version_info[0], int)
assert isinstance(sys.version_info[1], int)
assert isinstance(sys.version_info[2], int)
assert isinstance(sys.version_info[3], str)
assert isinstance(sys.version_info[4], int)

# Test negative indexing
assert sys.version_info[-1] == sys.version_info[4]
assert sys.version_info[-2] == sys.version_info[3]
assert sys.version_info[-5] == sys.version_info[0]

# Test named attribute access matches index access
assert sys.version_info.major == sys.version_info[0]
assert sys.version_info.minor == sys.version_info[1]
assert sys.version_info.micro == sys.version_info[2]
assert sys.version_info.releaselevel == sys.version_info[3]
assert sys.version_info.serial == sys.version_info[4]

# Test len
assert len(sys.version_info) == 5

# Test tuple equality (works after fixing NamedTuple equality)
v = sys.version_info
assert (v[0], v[1]) == (v.major, v.minor)
assert v.major == v[0]
assert v.minor == v[1]

# === sys.platform ===
# Check that platform is a non-empty string (exact value differs between interpreters)
assert isinstance(sys.platform, str)
assert len(sys.platform) > 0

# === sys.stdout and sys.stderr ===
# These should exist - we test by accessing them (will fail if not present)
stdout = sys.stdout
stderr = sys.stderr
