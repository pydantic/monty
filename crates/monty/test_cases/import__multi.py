# Tests for multi-module import statements (import a, b, c)

# === Basic multi-module import ===
import sys, math

assert isinstance(sys.version, str), 'sys should be accessible after multi-import'
assert math.pi > 3.14, 'math should be accessible after multi-import'

# === Multi-module import with alias ===
import sys as s, math as m

assert isinstance(s.version, str), 'sys alias should work in multi-import'
assert m.pi > 3.14, 'math alias should work in multi-import'

# === Mixed alias and non-alias ===
import sys, math as m2

assert isinstance(sys.version, str), 'non-aliased module should work in mixed import'
assert m2.pi > 3.14, 'aliased module should work in mixed import'
