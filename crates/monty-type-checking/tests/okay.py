from typing import assert_type


def get_int() -> int:
    return 123


def get_float() -> float:
    return 3.14


def get_str() -> str:
    return 'hello'


def get_list_int() -> list[int]:
    return [1, 2, 3]


def get_list_str() -> list[str]:
    return ['a', 'b', 'c']


def get_object() -> object:
    return object()


def get_dict_str_int() -> dict[str, int]:
    return {'a': 1, 'b': 2}


def get_set_str() -> set[str]:
    return {'a', 'b', 'c'}


def get_frozenset_str() -> frozenset[str]:
    return frozenset({'a', 'b', 'c'})


def get_tuple_str_int() -> tuple[str, int]:
    return ('hello', 42)


def get_bytes() -> bytes:
    return b'hello'


# === Type Constructors ===

# int
a = int(get_int())
assert_type(a, int)
b = int(get_float())
assert_type(b, int)
f = get_float()
assert_type(f, float)

# str
g = str(get_int())
assert_type(g, str)

# bool
i = bool(get_int())
assert_type(i, bool)

# bytes
k = bytes(get_int())
assert_type(k, bytes)
k2 = get_bytes()
assert_type(k2, bytes)

# range
w = range(get_int())
assert_type(w, range)
x = range(0, get_int(), 2)
assert_type(x, range)

# === Compound Types ===

# list[int]
m = list(get_list_int())
assert_type(m, list[int])
m2 = get_list_int()
assert_type(m2, list[int])

# list[str]
m3 = get_list_str()
assert_type(m3, list[str])

# tuple[int, ...]
p = tuple(get_list_int())
assert_type(p, tuple[int, ...])

# tuple[str, int]
p2 = get_tuple_str_int()
assert_type(p2, tuple[str, int])

# dict[str, int]
d = get_dict_str_int()
assert_type(d, dict[str, int])

# set[int]
s = set(get_list_int())
assert_type(s, set[int])

# set[str]
s2 = get_set_str()
assert_type(s2, set[str])

# frozenset[int]
u = frozenset(get_list_int())
assert_type(u, frozenset[int])

# frozenset[str]
u2 = get_frozenset_str()
assert_type(u2, frozenset[str])


# === Builtin Functions ===

# all / any
aa = all(get_list_int())
assert_type(aa, bool)
ab = any(get_list_int())
assert_type(ab, bool)

# bin / hex / oct
ac = bin(get_int())
assert_type(ac, str)
ad = hex(get_int())
assert_type(ad, str)
ae = oct(get_int())
assert_type(ae, str)

# chr / ord
af = chr(get_int())
assert_type(af, str)
ag = ord(get_str())
assert_type(ag, int)

# hash
ai = hash(get_str())
assert_type(ai, int)
aj = hash(get_int())
assert_type(aj, int)

# id
ak = id(get_object())
assert_type(ak, int)

# isinstance - use object to avoid literal True inference
al = isinstance(get_object(), int)
assert_type(al, bool)

# len
an = len(get_list_int())
assert_type(an, int)
ao = len(get_str())
assert_type(ao, int)

# min / max
ap = min(get_int(), get_int())
assert_type(ap, int)
aq = max(get_int(), get_int())
assert_type(aq, int)
ar = min(get_list_int())
assert_type(ar, int)
as_ = max(get_list_int())
assert_type(as_, int)

# print
aw = print(get_str())
assert_type(aw, None)

# repr
ax = repr(get_int())
assert_type(ax, str)

# sum
ba = sum(get_list_int())
assert_type(ba, int)
bc = sum(get_list_int(), get_int())
assert_type(bc, int)

# sorted
bd = sorted(get_list_int())
assert_type(bd, list[int])

# type
bf = type(get_int())
assert_type(bf, type[int])
bg = type(get_str())
assert_type(bg, type[str])

# enumerate
bh = enumerate(get_list_int())
assert_type(bh, enumerate[int])

# reversed
bi = reversed(get_list_int())
assert_type(bi, reversed[int])


# === Literal types ===

# None
bk = None
assert_type(bk, None)
