# /// monty
# mount-fs = true
# ///

import sys
from collections import namedtuple


# === Scope ===
def outer(captured):
    def inner(arg):
        if arg:
            unbound = captured
        return captured, dir()

    return inner(False)


assert outer(1) == (1, ['arg', 'captured'])
assert eval('dir()', {}, {'z': 1, 'a': 2}) == ['a', 'z']
assert dir() == sorted(dir())

try:
    dir(o=None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'dir() takes no keyword arguments'

try:
    dir(1, 2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'dir expected at most 1 argument, got 2'


# === Classes and instances ===
class Base:
    inherited = 1

    def name(self):
        return 'base'


class Child(Base):
    def name(self):
        return 'child'


child = Child()
child.own = 3
child.name = 'shadowed'
assert [name for name in dir(child) if not name.startswith('_')] == ['inherited', 'name', 'own']
assert [name for name in dir(Child) if not name.startswith('_')] == ['inherited', 'name']
assert 'append' in dir([])
assert 'strip' in dir(str)
assert '__mro__' not in dir(str)
assert 'child' not in dir(None)

long_name = 'runtime_attribute_' + 'x' * 32
setattr(Child, long_name, 1)
setattr(child, long_name, 2)
assert dir(child).count(long_name) == 1

Point = namedtuple('Point', 'x y')
assert 'x' in dir(Point)
assert 'y' in dir(Point(1, 2))
assert '_asdict' in dir(Point(1, 2))
assert 'major' in dir(sys.version_info)
assert '_asdict' not in dir(sys.version_info)


# === Special method lookup ===
class WithDir:
    def __dir__(self):
        return ('z', 'a', 'z')


hooked = WithDir()
hooked.__dir__ = lambda: ['instance']
assert dir(hooked) == ['a', 'z', 'z']
assert '__dir__' in dir(WithDir)

# === File modules ===
(root / 'dir_fixture.py').write_text('z = 1\na = 2\n')
(root / 'dir_hook_fixture.py').write_text('def __dir__():\n    return ["z", "a"]\n')
import dir_fixture
import dir_hook_fixture

assert [name for name in dir(dir_fixture) if not name.startswith('_')] == ['a', 'z']
assert dir(dir_hook_fixture) == ['a', 'z']
