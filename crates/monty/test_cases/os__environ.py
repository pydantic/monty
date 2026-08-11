# call-external
# Tests for os.environ property

import os

# === os.environ property ===
# os.environ returns a dict-like object
env = os.environ

# === os.environ key access ===
assert env['VIRTUAL_HOME'] == '/virtual/home'
assert os.environ['VIRTUAL_HOME'] == '/virtual/home'
assert os.environ['VIRTUAL_USER'] == 'testuser'
assert os.environ['VIRTUAL_EMPTY'] == ''

# === os.environ get method ===
assert env.get('VIRTUAL_HOME') == '/virtual/home'
assert os.environ.get('VIRTUAL_HOME') == '/virtual/home'
assert os.environ.get('VIRTUAL_USER') == 'testuser'
assert os.environ.get('NONEXISTENT_VAR_12345') is None
assert os.environ.get('NONEXISTENT_VAR_12345', 'default') == 'default'

# === os.environ length ===
assert len(env) == 3

# === os.environ membership test ===
assert 'VIRTUAL_HOME' in env
assert 'VIRTUAL_HOME' in os.environ
assert 'VIRTUAL_USER' in env
assert 'NONEXISTENT_VAR_12345' not in env
assert 'NONEXISTENT_VAR_12345' not in os.environ

# === os.environ keys/values/items ===
keys = list(os.environ.keys())
assert 'VIRTUAL_HOME' in keys
assert 'VIRTUAL_USER' in keys

values = list(os.environ.values())
assert '/virtual/home' in values
assert 'testuser' in values

try:
    os.getenv(None)
    assert False, 'str expected, not None'
except TypeError as e:
    assert str(e) == 'str expected, not NoneType'

try:
    os.getenv([1, 2, 3])
    assert False, 'str expected, not list'
except TypeError as e:
    assert str(e) == 'str expected, not list'

try:
    os.getenv(123)
    assert False, 'str expected, not int'
except TypeError as e:
    assert str(e) == 'str expected, not int'
