# Tests for multi-module import statements (import a, b, c)

# === Basic multi-module import ===
import sys, math

assert isinstance(sys.version, str)
assert math.pi > 3.14

# === Multi-module import with alias ===
import sys as s, math as m

assert isinstance(s.version, str)
assert m.pi > 3.14

# === Mixed alias and non-alias ===
import sys, math as m2

assert isinstance(sys.version, str)
assert m2.pi > 3.14
