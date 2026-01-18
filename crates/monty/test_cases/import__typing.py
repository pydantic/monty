# Tests for typing module import

# === TYPE_CHECKING ===
from typing import TYPE_CHECKING

assert TYPE_CHECKING == False, 'TYPE_CHECKING should be False'
assert not TYPE_CHECKING, 'TYPE_CHECKING should be falsy'
