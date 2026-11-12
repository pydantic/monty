# /// monty
# call-external = true
# ///
# `from m import *` for modules whose attributes involve the host; module files are in `import__star.py`.

# === a host module binds its attributes ===
from tools.nested import *

assert VERSION == 7
assert return_value([1]) == [1]

# === a property that asks the host: `os.environ` ===
ns = {}
exec('from os import *', ns)
assert ns['environ']['VIRTUAL_HOME'] == '/virtual/home'
assert ns['getenv']('VIRTUAL_USER') == 'testuser'

# === the same, binding module globals ===
from os import *

assert environ['VIRTUAL_USER'] == 'testuser'
assert getenv('VIRTUAL_HOME') == '/virtual/home'
