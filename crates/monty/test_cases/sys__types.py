# Tests for sys module types

import sys

# === Verify type() returns _io.TextIOWrapper for stdout/stderr ===
assert str(type(sys.stdout)) == "<class '_io.TextIOWrapper'>", 'type(stdout) is _io.TextIOWrapper'
assert str(type(sys.stderr)) == "<class '_io.TextIOWrapper'>", 'type(stderr) is _io.TextIOWrapper'
