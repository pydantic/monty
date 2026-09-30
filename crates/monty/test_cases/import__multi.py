# Tests for multi-module import statements (import a, b, c)

import base64
import sys, math, re

# Avoid a module attribute here: mentioning one would independently intern its
# name and could hide omissions from the module's registration list.
assert base64 is not None, 'base64 import should succeed'

# === Basic multi-module import ===

assert isinstance(sys.version, str)
assert math.pi > 3.14
assert math.e > 2.71
assert re.A == 256
assert re.I == 2
assert re.M == 8
assert re.S == 16

# === Multi-module import with alias ===
import sys as s, math as m

assert isinstance(s.version, str)
assert m.pi > 3.14

# === Mixed alias and non-alias ===
import sys, math as m2

assert isinstance(sys.version, str)
assert m2.pi > 3.14
