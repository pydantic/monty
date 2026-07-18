# __repr__ / __str__ dispatch for user-defined classes, including use inside
# repr()/str()/f-strings/print and nested containers, plus the default repr.


class Vec:
    def __init__(self, x: int, y: int) -> None:
        self.x = x
        self.y = y

    def __repr__(self) -> str:
        return f'Vec({self.x}, {self.y})'


v = Vec(1, 2)

# === repr() / str() dispatch to __repr__ ===
assert repr(v) == 'Vec(1, 2)'
assert str(v) == 'Vec(1, 2)'

# === f-strings ===
assert f'{v!r}' == 'Vec(1, 2)'
assert f'{v}' == 'Vec(1, 2)'

# === repr inside containers uses __repr__ ===
assert repr([v, v]) == '[Vec(1, 2), Vec(1, 2)]'
assert repr((v,)) == '(Vec(1, 2),)'
assert repr({'k': v}) == "{'k': Vec(1, 2)}"


class Temperature:
    def __init__(self, celsius: int) -> None:
        self.celsius = celsius

    def __repr__(self) -> str:
        return f'Temperature(celsius={self.celsius})'

    def __str__(self) -> str:
        return f'{self.celsius}°C'


t = Temperature(20)

# === __str__ overrides __repr__ for str()/print, repr() still uses __repr__ ===
assert str(t) == '20°C'
assert repr(t) == 'Temperature(celsius=20)'
assert f'{t}' == '20°C'
assert f'{t!r}' == 'Temperature(celsius=20)'

# === __repr__ that references other instances ===


class Node:
    def __init__(self, value: int, nxt) -> None:
        self.value = value
        self.nxt = nxt

    def __repr__(self) -> str:
        return f'Node({self.value}, {self.nxt!r})'


chain = Node(1, Node(2, None))
assert repr(chain) == 'Node(1, Node(2, None))'

# === Default repr (no __repr__): bare class name + address ===
# CPython qualifies the name (`<__main__.Plain object ...>`); Monty uses the bare
# class name. Both contain "Plain object at" and an address, so check loosely.


class Plain:
    def __init__(self) -> None:
        self.a = 1


pl = Plain()
assert repr(pl).startswith('<Plain object at 0x')
assert str(pl) == repr(pl)
assert type(pl).__name__ == 'Plain'

# === __repr__/__str__ set to an already-bound method: no extra `self` is bound ===
# `Bar.__repr__` here is a `BoundMethod`, not a plain function, so dispatch must
# not re-inject `self` (unlike descriptor binding for a plain-function member).


class Greeter:
    def greet(self) -> str:
        return 'hi'


class Bar:
    __repr__ = Greeter().greet
    __str__ = Greeter().greet


assert repr(Bar()) == 'hi'
assert str(Bar()) == 'hi'
