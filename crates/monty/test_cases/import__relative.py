# /// monty
# mount-fs = true
# ///
# Every file is written before the first import: CPython caches directory listings.
(root / 'rel').mkdir()
(root / 'rel' / '__init__.py').write_text(
    'from . import a\nfrom .a import A\nfrom .sub import deep\nfrom .sub.deep import D\nfrom . import a as alias\n'
)
(root / 'rel' / 'a.py').write_text('A = 1\n')
(root / 'rel' / 'b.py').write_text('from .a import A as B\n')
(root / 'rel' / 'lazy.py').write_text('def load():\n    from .b import B\n    return B\n')
(root / 'rel' / 'sub').mkdir()
(root / 'rel' / 'sub' / '__init__.py').write_text('')
(root / 'rel' / 'sub' / 'deep.py').write_text(
    'D = 2\nfrom .. import a as parent_a\nfrom ..a import A\nfrom ..sub import leaf\nfrom . import sibling\n'
)
(root / 'rel' / 'sub' / 'leaf.py').write_text('L = 3\n')
(root / 'rel' / 'sub' / 'sibling.py').write_text('from .leaf import L\n')
(root / 'rel' / 'beyond.py').write_text('from ... import x\n')
(root / 'rel' / 'sub' / 'beyond.py').write_text('from .... import x\n')
(root / 'rel' / 'nomod.py').write_text('from .nope import x\n')
(root / 'rel' / 'nosub.py').write_text('from .a.nope import x\n')
(root / 'rel' / 'noname.py').write_text('from .a import nope\n')
(root / 'rel' / 'noattr.py').write_text('from . import nope\n')
(root / 'rel' / 'fails.py').write_text('from . import broken\n')
(root / 'rel' / 'broken.py').write_text("raise ValueError('broken')\n")
(root / 'top.py').write_text('from . import x\n')
(root / 'circ').mkdir()
(root / 'circ' / '__init__.py').write_text('from . import mod\nlate = 1\n')
(root / 'circ' / 'mod.py').write_text('try:\n    from . import late\nexcept ImportError as e:\n    msg = str(e)\n')

# === Relative imports of submodules, names and parent packages ===
import rel

assert (rel.A, rel.D) == (1, 2)
assert rel.a is rel.alias
assert rel.deep is rel.sub.deep
assert rel.sub.deep.parent_a is rel.a
assert (rel.sub.deep.A, rel.sub.deep.leaf.L, rel.sub.deep.sibling.L) == (1, 3, 3)
import rel.b

assert rel.b.B == 1

# === A relative import in a function resolves when it runs ===
import rel.lazy

assert rel.lazy.load() == 1

# === Circular imports name the partly initialised package ===
import circ

assert circ.mod.msg == (
    "cannot import name 'late' from partially initialized module 'circ' "
    f'(most likely due to a circular import) ({root / "circ" / "__init__.py"})'
)

# === Relative imports above the top-level package ===
try:
    import rel.beyond

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == 'attempted relative import beyond top-level package'
try:
    import rel.sub.beyond

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == 'attempted relative import beyond top-level package'

# === A top-level module has no parent package ===
try:
    import top

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == 'attempted relative import with no known parent package'

# === Missing modules and names ===
try:
    import rel.nomod

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'rel.nope'"
try:
    import rel.nosub

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'rel.a.nope'; 'rel.a' is not a package"
try:
    import rel.noname

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == f"cannot import name 'nope' from 'rel.a' ({root / 'rel' / 'a.py'})"
try:
    import rel.noattr

    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == f"cannot import name 'nope' from 'rel' ({root / 'rel' / '__init__.py'})"

# === A failing submodule propagates its error ===
try:
    import rel.fails

    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'broken'

# === exec() resolves against its globals' `__package__`, else `__name__` ===
g = {'__package__': 'rel'}
exec('from . import a\nfrom .sub.deep import D', g)
assert (g['a'], g['D']) == (rel.a, 2)
g = {'__package__': 'rel.sub'}
exec('from .. import a', g)
assert g['a'] is rel.a
g = {'__name__': 'rel.x'}
exec('from . import a', g)
assert g['a'] is rel.a
g = {'__name__': 'rel', '__path__': []}
exec('from . import a', g)
assert g['a'] is rel.a
g = {'__package__': None, '__name__': 'rel.y'}
exec('from . import a', g)
assert g['a'] is rel.a
try:
    exec('from . import a', {'__package__': ''})
    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == 'attempted relative import with no known parent package'
try:
    exec('from .. import a', {'__package__': 'rel'})
    assert False, 'expected ImportError'
except ImportError as e:
    assert str(e) == 'attempted relative import beyond top-level package'
try:
    exec('from . import a', {'__package__': 3})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'package must be a string'
try:
    exec('from . import a', {'__name__': 3})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '__name__ must be a string'
try:
    exec('from . import a', {})
    assert False, 'expected KeyError'
except KeyError as e:
    assert e.args == ("'__name__' not in globals",)

# === A built-in module can be the package ===
g = {'__package__': 'os', '__name__': 'x'}
exec('from . import path\nfrom .path import basename', g)
assert (g['path'].basename('a/b'), g['basename']('c/d')) == ('b', 'd')
g = {'__package__': 'os.path', '__name__': 'x'}
exec('from . import basename', g)
assert g['basename']('a/b') == 'b'
try:
    exec('from .nope import x', {'__package__': 'os'})
    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as e:
    assert str(e) == "No module named 'os.nope'; 'os' is not a package"
