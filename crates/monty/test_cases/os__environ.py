# call-external
# Tests for os.environ property

import os

# === os.environ property ===
# os.environ returns a dict-like object
env = os.environ

# === os.environ key access ===
assert env['VIRTUAL_HOME'] == '/virtual/home', 'environ key access VIRTUAL_HOME'
assert os.environ['VIRTUAL_HOME'] == '/virtual/home', 'environ key access VIRTUAL_HOME'
assert os.environ['VIRTUAL_USER'] == 'testuser', 'environ key access VIRTUAL_USER'
assert os.environ['VIRTUAL_EMPTY'] == '', 'environ key access VIRTUAL_EMPTY'

# === os.environ get method ===
assert env.get('VIRTUAL_HOME') == '/virtual/home', 'environ.get existing key'
assert os.environ.get('VIRTUAL_HOME') == '/virtual/home', 'environ.get existing key'
assert os.environ.get('VIRTUAL_USER') == 'testuser', 'environ.get existing user'
assert os.environ.get('NONEXISTENT_VAR_12345') is None, 'environ.get missing returns None'
assert os.environ.get('NONEXISTENT_VAR_12345', 'default') == 'default', 'environ.get with default'

# === os.environ length ===
assert len(env) == 3, 'environ has 3 virtual entries'

# === os.environ membership test ===
assert 'VIRTUAL_HOME' in env, 'VIRTUAL_HOME in environ'
assert 'VIRTUAL_HOME' in os.environ, 'VIRTUAL_HOME in environ'
assert 'VIRTUAL_USER' in env, 'VIRTUAL_USER in environ'
assert 'NONEXISTENT_VAR_12345' not in env, 'nonexistent not in environ'
assert 'NONEXISTENT_VAR_12345' not in os.environ, 'nonexistent not in environ'

# === os.environ keys/values/items ===
keys = list(os.environ.keys())
assert 'VIRTUAL_HOME' in keys, 'VIRTUAL_HOME in keys'
assert 'VIRTUAL_USER' in keys, 'VIRTUAL_USER in keys'

values = list(os.environ.values())
assert '/virtual/home' in values, '/virtual/home in values'
assert 'testuser' in values, 'testuser in values'
