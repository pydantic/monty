# /// monty
# mount-fs = true
# ///
# Every file is written before the first import: CPython caches directory listings.
(root / 'foo.py').write_text('x = 41\ndef f():\n    return x + 1\n')
(root / 'dunders.py').write_text('name = __name__\nfile = __file__\n')
(root / 'counter.py').write_text('items = []\nitems.append(len(items))\n')
(root / 'a.py').write_text('first = 1\nimport b\nlast = b.seen\n')
(root / 'b.py').write_text("import a\nseen = a.first\nhas_last = hasattr(a, 'last')\n")
(root / 'attempts.py').write_text('count = 0\n')
(root / 'flaky.py').write_text(
    "import attempts\nattempts.count += 1\nif attempts.count == 1:\n    raise ValueError('first time')\n"
)
(root / 'helpers.py').write_text('def run():\n    return fetch()\n')
(root / 'pkg').mkdir()
(root / 'pkg' / '__init__.py').write_text("top = 'pkg'\n")
(root / 'pkg' / 'sub.py').write_text('value = 3\n')
(root / 'pkg' / 'inner').mkdir()
(root / 'pkg' / 'inner' / '__init__.py').write_text('')
(root / 'pkg' / 'inner' / 'deep.py').write_text('value = 4\n')
(root / 'both.py').write_text("where = 'module'\n")
(root / 'both').mkdir()
(root / 'both' / '__init__.py').write_text("where = 'package'\n")
(root / 'both' / 'sub.py').write_text('value = 6\n')
(root / 'elsewhere').mkdir()
(root / 'elsewhere' / 'far.py').write_text('value = 5\n')
(root / 'redirect').mkdir()
(root / 'redirect' / '__init__.py').write_text(f'__path__ = ({str(root / "elsewhere")!r},)\n')
(root / 'mixed').mkdir()
(root / 'mixed' / '__init__.py').write_text(f'__path__ = [1, None, {str(root / "elsewhere")!r}]\n')
(root / 'circ_a.py').write_text('import circ_b\nlate = 1\n')
(root / 'circ_b.py').write_text('try:\n    from circ_a import late\nexcept ImportError as e:\n    msg = str(e)\n')
(root / 'cpkg').mkdir()
(root / 'cpkg' / '__init__.py').write_text('from cpkg import mod\nlate = 1\n')
(root / 'cpkg' / 'mod.py').write_text('try:\n    from cpkg import late\nexcept ImportError as e:\n    msg = str(e)\n')

# === Modules ===
import foo

assert foo.f() == 42
import foo as m

assert m is foo
from foo import f, x as y

assert (f(), y) == (42, 41)
assert foo.__name__ == 'foo'
assert foo.__file__ == str(root / 'foo.py')
assert foo.__package__ == ''
assert repr(foo) == f'<module {"foo"!r} from {str(root / "foo.py")!r}>'

import dunders

assert (dunders.name, dunders.file) == ('dunders', str(root / 'dunders.py'))

# === Attribute writes rebind the module's globals ===
foo.x = 1
assert foo.f() == 2
foo.z = 5
assert foo.z == 5

# === A module runs once ===
import counter
import counter as again

assert again is counter
assert counter.items == [0]

# === Circular imports bind the partly initialised module ===
import a

assert (a.last, a.b.has_last) == (1, False)

# === `from` a partly initialised module names the circular import ===
import circ_a

assert circ_a.circ_b.msg == (
    "cannot import name 'late' from partially initialized module 'circ_a' "
    f'(most likely due to a circular import) ({root / "circ_a.py"})'
)
import cpkg

assert cpkg.mod.msg == (
    "cannot import name 'late' from partially initialized module 'cpkg' "
    f'(most likely due to a circular import) ({root / "cpkg" / "__init__.py"})'
)

# === A raising module is not cached ===
try:
    import flaky

    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'first time'
import flaky
import attempts

assert attempts.count == 2

# === Module code sees only its own globals and the builtins ===
import helpers

try:
    helpers.run()
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'fetch' is not defined"
helpers.fetch = lambda: 42
assert helpers.run() == 42

# === Packages ===
import pkg

assert pkg.top == 'pkg'
import pkg.sub

assert pkg.sub.value == 3
import pkg.sub as s

assert s is pkg.sub
from pkg import sub

assert sub is pkg.sub
from pkg.sub import value

assert value == 3
import pkg.inner.deep

assert pkg.inner.deep.value == 4
assert pkg.__path__ == [str(root / 'pkg')]
assert pkg.__file__ == str(root / 'pkg' / '__init__.py')
assert (pkg.__package__, sub.__package__) == ('pkg', 'pkg')

# === A package wins over a module of the same name ===
import both.sub

assert (both.where, both.sub.value) == ('package', 6)

# === `__path__` may be a tuple, and its non-strings are skipped ===
import redirect.far

assert redirect.far.value == 5
import mixed.far

assert mixed.far.value == 5

# === Missing names and submodules ===
try:
    from foo import nope

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == f"cannot import name 'nope' from 'foo' ({root / 'foo.py'})"
try:
    from pkg import nope

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == f"cannot import name 'nope' from 'pkg' ({root / 'pkg' / '__init__.py'})"
try:
    import foo.x

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'foo.x'; 'foo' is not a package"
try:
    import pkg.nope

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'pkg.nope'"
try:
    import nope

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'nope'"
try:
    foo.nope
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "module 'foo' has no attribute 'nope'"
