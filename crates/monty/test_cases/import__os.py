# call-external
# Tests for os module import and os.getenv()

import os

# === os.getenv() with existing variable ===
assert os.getenv('VIRTUAL_HOME') == '/virtual/home', 'getenv returns existing value'
assert os.getenv('VIRTUAL_USER') == 'testuser', 'getenv returns user value'
assert os.getenv('VIRTUAL_EMPTY') == '', 'getenv returns empty string value'

# === os.getenv() with missing variable ===
assert os.getenv('NONEXISTENT') is None, 'getenv returns None for missing var'
assert os.getenv('ALSO_MISSING') is None, 'getenv returns None for other missing var'

# === os.getenv() with default value ===
assert os.getenv('NONEXISTENT', 'fallback') == 'fallback', 'getenv uses default when missing'
assert os.getenv('ALSO_MISSING', '') == '', 'getenv uses empty string default'
assert os.getenv('MISSING', None) is None, 'getenv with explicit None default'

# === os.getenv() existing var ignores default ===
assert os.getenv('VIRTUAL_HOME', 'ignored') == '/virtual/home', 'existing var ignores default'
assert os.getenv('VIRTUAL_USER', 'other') == 'testuser', 'existing user ignores default'

# === os.getenv() with empty string existing var ===
assert os.getenv('VIRTUAL_EMPTY', 'not_used') == '', 'empty string var ignores default'
