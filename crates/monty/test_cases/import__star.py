# /// monty
# mount-fs = true
# ///
# `from m import *` binds `m.__all__`, or every name of `m` not starting with `_`.
# Every file is written before the first import: CPython caches directory listings.
(root / 'plain.py').write_text(
    "a = 1\nb = 'two'\n_hidden = 3\n__dunder__ = 4\nimport json\ndef f():\n    return a + 1\n"
)
(root / 'listed.py').write_text("__all__ = ['x', '_y']\nx = 10\n_y = 20\nz = 30\n")
(root / 'tupled.py').write_text("__all__ = ('p',)\np = 'p'\nq = 'q'\n")
(root / 'stringed.py').write_text("__all__ = 'uv'\nu = 1\nv = 2\nw = 3\n")
(root / 'execd.py').write_text('from_exec = 42\n')
(root / 'nested_star.py').write_text(
    'from listed import *\nfrom math import *\nseen = (x, _y, floor(1.5))\ndef g():\n    return x + _y\n'
)
(root / 'bad_item.py').write_text("__all__ = ['ok', 1]\nok = 'ok'\n")
(root / 'missing.py').write_text("__all__ = ['there', 'gone']\nthere = 'there'\n")
(root / 'none_all.py').write_text('__all__ = None\n')
(root / 'dict_all.py').write_text("__all__ = {'k': 1}\nk = 1\n")
(root / 'ranged.py').write_text('__all__ = range(0)\n')
(root / 'bytes_all.py').write_text("__all__ = b'a'\na = 1\n")
(root / 'deque_all.py').write_text("import collections\n__all__ = collections.deque(['dq'])\ndq = 'dq'\n")
(root / 'nt_all.py').write_text(
    "import collections\nP = collections.namedtuple('P', 'x')\n__all__ = P('nt')\nnt = 'nt'\n"
)
(root / 'counter_all.py').write_text(
    "import collections\n__all__ = collections.Counter({0: 'c0', 1: 'c1'})\nc0 = 0\nc1 = 1\n"
)
(root / 'getitem_all.py').write_text(
    'class Names:\n'
    '    def __getitem__(self, i):\n'
    '        if i < 2:\n'
    "            return 'g' + str(i)\n"
    "        raise IndexError('done')\n"
    '__all__ = Names()\n'
    "g0 = 'g0'\n"
    "g1 = 'g1'\n"
)
(root / 'getitem_raises.py').write_text(
    "class Names:\n    def __getitem__(self, i):\n        raise KeyError('k')\n__all__ = Names()\n"
)
(root / 'iter_all.py').write_text(
    "class Names:\n    def __iter__(self):\n        return iter(['a'])\n__all__ = Names()\na = 1\n"
)
(root / 'set_all.py').write_text("__all__ = {'a'}\na = 1\n")
(root / 'iterator_all.py').write_text("__all__ = iter(['a'])\na = 1\n")
(root / 'dd_all.py').write_text('import collections\n__all__ = collections.defaultdict(int)\n')
(root / 'renamed.py').write_text("__name__ = 'other'\n__all__ = [1]\n")
(root / 'renamed_missing.py').write_text("__name__ = 'other'\n__all__ = ['gone']\n")
(root / 'int_named.py').write_text('__name__ = 5\n__all__ = [1]\n')
(root / 'shadow.py').write_text("__all__ = ['len']\ndef len(o):\n    return 'shadowed'\n")
(root / 'spkg').mkdir()
(root / 'spkg' / '__init__.py').write_text('')
(root / 'spkg' / 'mod.py').write_text("__all__ = ['deep']\ndeep = 'deep'\n")


# === a function defined before the import reads the bound global ===
def early():
    return trunc(2.7)


# === bundled module without `__all__` ===
from math import *

assert sqrt(16) == 4.0
# `math.pow` replaces the builtin, which always returns `int` for `int` operands
assert type(pow(2, 3)) is float
assert floor(2.5) == 2
assert early() == 2

# === bundled submodule ===
from os.path import *

assert join('a', 'b') == 'a/b'
assert basename('/x/y.txt') == 'y.txt'

# === module file without `__all__`: names starting with `_` are skipped ===
from plain import *

assert (a, b, f()) == (1, 'two', 2)
assert json.dumps([1]) == '[1]'
try:
    _hidden
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name '_hidden' is not defined"
try:
    __dunder__
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name '__dunder__' is not defined"

# === `__all__` selects the names, private ones included ===
from listed import *

assert (x, _y) == (10, 20)
try:
    z
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'z' is not defined"

# === `__all__` may be any sequence of strings ===
from tupled import *

assert p == 'p'
try:
    q
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'q' is not defined"
from stringed import *

assert (u, v) == (1, 2)
try:
    w
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'w' is not defined"

# === a module package's submodule ===
from spkg.mod import *

assert deep == 'deep'


# === functions defined after the import ===
def later():
    return sqrt(9) + x


assert later() == 13.0

# === a module body sees its own star imports, and so do its functions ===
from nested_star import seen, g

assert seen == (10, 20, 1)
assert g() == 30

# === exec() at module level binds module globals ===
exec('from execd import *')
assert from_exec == 42

# === exec() with a globals dict, and with a locals dict ===
ns = {}
exec('from listed import *', ns)
assert (ns['x'], ns['_y'], 'z' in ns) == (10, 20, False)


def star_into_locals():
    loc = {}
    exec('from tupled import *', {}, loc)
    return loc


assert star_into_locals() == {'p': 'p'}

# === only allowed at module level ===
try:
    exec('def h():\n    from math import *')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e) == 'import * only allowed at module level (<string>, line 2)'
try:
    exec('class C:\n    from math import *')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e) == 'import * only allowed at module level (<string>, line 2)'

# === a non-`str` entry raises, after binding the entries before it ===
try:
    from bad_item import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Item in bad_item.__all__ must be str, not int'
assert ok == 'ok'

# === a missing name raises, after binding the names before it ===
try:
    from missing import *

    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "module 'missing' has no attribute 'gone'"
assert there == 'there'

# === `__all__` that is not a sequence ===
try:
    from none_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'NoneType' object does not support indexing"
try:
    from dict_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'dict is not a sequence'

# === `__all__` is indexed from 0 until `IndexError`, so any sequence works ===
from ranged import *
from deque_all import *
from nt_all import *
from getitem_all import *

assert (dq, nt, g0, g1) == ('dq', 'nt', 'g0', 'g1')
try:
    from bytes_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Item in bytes_all.__all__ must be str, not int'
# a `Counter` answers every index: missing keys count 0
try:
    from counter_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Item in counter_all.__all__ must be str, not int'
assert (c0, c1) == (0, 1)
try:
    from getitem_raises import *

    assert False, 'expected KeyError'
except KeyError as e:
    assert str(e) == "'k'"

# === an iterable is not enough, nor is a mapping ===
try:
    from iter_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'Names' object does not support indexing"
try:
    from set_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'set' object does not support indexing"
try:
    from iterator_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'list_iterator' object does not support indexing"
try:
    from dd_all import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'collections.defaultdict is not a sequence'

# === errors name the module by its `__name__` ===
try:
    from renamed import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Item in other.__all__ must be str, not int'
try:
    from renamed_missing import *

    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "module 'other' has no attribute 'gone'"
try:
    from int_named import *

    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'module __name__ must be a string, not int'
# as do attribute errors outside star imports
import int_named
import renamed_missing

try:
    renamed_missing.absent
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "module 'other' has no attribute 'absent'"
try:
    int_named.absent()
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "module has no attribute 'absent'"

# === a star import can rebind a builtin at module level ===
assert len([1]) == 1
from shadow import *

assert len([1]) == 'shadowed'
