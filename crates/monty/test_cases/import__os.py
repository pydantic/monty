# call-external
# Tests for os module import and os.getenv()

import os

# === os.getenv() with existing variable ===
assert os.getenv('VIRTUAL_HOME') == '/virtual/home'
assert os.getenv('VIRTUAL_USER') == 'testuser'
assert os.getenv('VIRTUAL_EMPTY') == ''

# === os.getenv() with missing variable ===
assert os.getenv('NONEXISTENT') is None
assert os.getenv('ALSO_MISSING') is None

# === os.getenv() with default value ===
assert os.getenv('NONEXISTENT', 'fallback') == 'fallback'
assert os.getenv('ALSO_MISSING', '') == ''
assert os.getenv('MISSING', None) is None

# === os.getenv() existing var ignores default ===
assert os.getenv('VIRTUAL_HOME', 'ignored') == '/virtual/home'
assert os.getenv('VIRTUAL_USER', 'other') == 'testuser'

# === os.getenv() with empty string existing var ===
assert os.getenv('VIRTUAL_EMPTY', 'not_used') == ''
