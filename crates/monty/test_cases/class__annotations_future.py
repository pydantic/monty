# Under `from __future__ import annotations` (PEP 563) a class body stores its
# annotations stringized, so `__annotations__` holds the text CPython's
# stringizer produces: unparsed, not sliced from the source.
from __future__ import annotations


# === Strings, in order, excluding unannotated names ===
class C:
    x: int
    y: str = 'hi'
    z = 5
    cv: ClassVar[int] = 0


assert list(C.__annotations__.keys()) == ['x', 'y', 'cv']
assert C.__annotations__['x'] == 'int'
assert C.__annotations__['y'] == 'str'
# Nothing is evaluated, so an unimported name is fine.
assert C.__annotations__['cv'] == 'ClassVar[int]'
assert C.__annotations__ is C.__annotations__


# === Annotations are normalized, not captured as raw source text ===
# Both unparse the expression, so spacing and line breaks are discarded.
class Spacing:
    a: list [ int ]  # fmt: skip
    b: dict[str,int]  # fmt: skip
    c: dict[
        str,
        int,
    ]


assert Spacing.__annotations__['a'] == 'list[int]'
assert Spacing.__annotations__['b'] == 'dict[str, int]'
assert Spacing.__annotations__['c'] == 'dict[str, int]'


# === String annotations normalize to single quotes, as CPython's does ===
class Quoted:
    a: "int"  # fmt: skip
    b: 'int'
    c: dict[str, "Foo"]  # fmt: skip
    # f-strings carry their own quote flags, so they normalize separately.
    d: f"int"  # fmt: skip
    # A single quote inside keeps the double quotes: escape-minimizing wins.
    e: "it's"  # fmt: skip


assert Quoted.__annotations__['a'] == "'int'"
assert Quoted.__annotations__['b'] == "'int'"
assert Quoted.__annotations__['c'] == "dict[str, 'Foo']"
assert Quoted.__annotations__['d'] == "f'int'"
assert Quoted.__annotations__['e'] == '"it\'s"'


# === Literals are rebuilt canonically, not echoed part-by-part ===
# Concatenated parts merge and a raw prefix folds into the value. A `u` prefix
# survives, though — canonical is not the same as bare.
class Literals:
    a: 'foo' 'bar'  # fmt: skip
    b: f"x" "y"  # fmt: skip
    c: r'raw\d'
    d: "a" r"b\n"  # fmt: skip
    e: u'uni'  # fmt: skip
    f: """triple"""  # fmt: skip
    g: f'pre{1}post'
    h: dict[str, 'A' 'B']  # fmt: skip


assert Literals.__annotations__['a'] == "'foobar'"
assert Literals.__annotations__['b'] == "f'xy'"
assert Literals.__annotations__['c'] == "'raw\\\\d'"
assert Literals.__annotations__['d'] == "'ab\\\\n'"
assert Literals.__annotations__['e'] == "u'uni'"
assert Literals.__annotations__['f'] == "'triple'"
assert Literals.__annotations__['g'] == "f'pre{1}post'"
assert Literals.__annotations__['h'] == "dict[str, 'AB']"


# === bytes literals canonicalize the same way, minus the `u` prefix ===
class ByteLiterals:
    a: b'foo'
    b: b'foo' b'bar'  # fmt: skip
    c: rb'raw\d'
    d: b"""triple"""  # fmt: skip


assert ByteLiterals.__annotations__['a'] == "b'foo'"
assert ByteLiterals.__annotations__['b'] == "b'foobar'"
assert ByteLiterals.__annotations__['c'] == "b'raw\\\\d'"
assert ByteLiterals.__annotations__['d'] == "b'triple'"


# === Functions keep an __annotate__ that returns the strings ===
def f(x: int, y: 'Foo') -> list[int]:
    pass


assert f.__annotations__ == {'x': 'int', 'y': "'Foo'", 'return': 'list[int]'}
assert f.__annotate__(1) == f.__annotations__


# === Empty class: __annotations__ is an empty dict ===
class E:
    p = 1


assert E.__annotations__ == {}


# === Assignment replaces the stored strings ===
class Replaced:
    x: int


Replaced.__annotations__ = {'y': str}
assert Replaced.__annotations__ == {'y': str}
