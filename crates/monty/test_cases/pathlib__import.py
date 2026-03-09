import pathlib

# Verify that pathlib.Path can be called as an attribute
p = pathlib.Path('a.txt')
assert p.name == 'a.txt'

# Verify that it still works when imported directly
from pathlib import Path

p2 = Path('b.txt')
assert p2.name == 'b.txt'
