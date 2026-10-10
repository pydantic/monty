# A dotted import that fails names the first missing component, or the module
# that turned out not to be a package, like CPython.

# === unknown top-level package ===
try:
    import a.b

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'a'"
try:
    import a.b.c

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'a'"
try:
    from a.b import c

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'a'"

# === a module that is not a package ===
try:
    import os.nothing

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'os.nothing'; 'os' is not a package"
try:
    import os.nothing.more

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'os.nothing'; 'os' is not a package"
try:
    import math.foo

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'math.foo'; 'math' is not a package"
try:
    import os.path.foo

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'os.path.foo'; 'os.path' is not a package"
try:
    from os.nothing import x

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'os.nothing'; 'os' is not a package"
