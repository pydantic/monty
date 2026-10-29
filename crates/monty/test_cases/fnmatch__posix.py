# /// monty
# skip-cpython-windows = true
# ///
# The sandbox's os.path is posixpath on every host, so fnmatch() and filter() never
# fold case or separators and a non-string name reaches the matcher unconverted.
# Windows CPython runs os.path.normcase on names first, so only the Monty side runs there.
import fnmatch

# === fnmatch() is case-sensitive, like fnmatchcase() ===
assert not fnmatch.fnmatch('FOO.py', '*.PY')
assert not fnmatch.fnmatch('foo.py', '*.PY')
assert fnmatch.filter(['a.py', 'B.PY', 'c.Py'], '*.py') == ['a.py']

# === separators are literal ===
assert not fnmatch.fnmatch('a\\b', 'a/b')
assert not fnmatch.fnmatch('a/b', 'a\\b')
assert fnmatch.filter(['a/b', 'a\\b'], 'a/*') == ['a/b']

# === a non-string name reaches the matcher, not os.fspath ===
try:
    fnmatch.filter(['a.py', 1], '*.py')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "expected string or bytes-like object, got 'int'"
