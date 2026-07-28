# Unary `-` and `+` across every type that implements them, and the types that
# do not (which raise CPython's `bad operand type` TypeError).
from collections import Counter, defaultdict
from datetime import timedelta

# === int and float ===
# through variables, so the operand is negated at runtime rather than folded
x = 5
assert -x == -5
assert +x == 5
assert -(-x) == 5
f = 2.5
assert -f == -2.5
assert +f == 2.5

# === bool promotes to int ===
assert -True == -1
assert +True == 1
assert -False == 0
assert type(+True).__name__ == 'int'
assert type(-False).__name__ == 'int'

# === the i64 boundary: negating the smallest int needs a big int ===
smallest = -9223372036854775808
assert -smallest == 9223372036854775808
assert type(-smallest).__name__ == 'int'
assert -(-smallest) == smallest

# === big ints ===
big = 2**70
assert -big == -1180591620717411303424
assert +big == 1180591620717411303424
assert -(-big) == big
assert +(-big) == -big
assert type(-big).__name__ == 'int'

# === timedelta ===
d = timedelta(days=1, seconds=2)
assert -d == timedelta(days=-1, seconds=-2)
assert +d == d
assert -(-d) == d
assert -timedelta(0) == timedelta(0)

# === Counter: both signs keep only the counts that end up positive ===
c = Counter(a=2, b=-1)
assert +c == Counter(a=2)
assert -c == Counter(b=1)
assert type(-c).__name__ == 'Counter'
# the operand itself is untouched — the result is a fresh Counter
assert c == Counter(a=2, b=-1)


def unary_err(fn):
    try:
        fn()
        raise AssertionError('expected TypeError')
    except TypeError as exc:
        return str(exc)


# === types with no unary form ===
assert unary_err(lambda: -'x') == "bad operand type for unary -: 'str'"
assert unary_err(lambda: +'x') == "bad operand type for unary +: 'str'"
# a heap-allocated (non-interned) string takes a different dispatch path
built = 'a' + str(1)
assert unary_err(lambda: -built) == "bad operand type for unary -: 'str'"
assert unary_err(lambda: -[1]) == "bad operand type for unary -: 'list'"
assert unary_err(lambda: +[1]) == "bad operand type for unary +: 'list'"
assert unary_err(lambda: -(1, 2)) == "bad operand type for unary -: 'tuple'"
assert unary_err(lambda: +{1: 2}) == "bad operand type for unary +: 'dict'"
assert unary_err(lambda: -{1, 2}) == "bad operand type for unary -: 'set'"
assert unary_err(lambda: -b'ab') == "bad operand type for unary -: 'bytes'"
assert unary_err(lambda: +None) == "bad operand type for unary +: 'NoneType'"
# a defaultdict is a dict of a different kind, so it has no Counter algebra
assert unary_err(lambda: +defaultdict(int)) == "bad operand type for unary +: 'collections.defaultdict'"
assert unary_err(lambda: -defaultdict(int)) == "bad operand type for unary -: 'collections.defaultdict'"
