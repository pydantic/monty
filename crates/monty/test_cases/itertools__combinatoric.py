# The combinatoric `itertools` iterators: `combinations`,
# `combinations_with_replacement`, `permutations` and `product`. Each collects
# its input up front and then steps indices into it, so the input is consumed
# once, at construction.
import itertools


# === combinations ===
assert list(itertools.combinations([1, 2, 3], 2)) == [(1, 2), (1, 3), (2, 3)]
assert list(itertools.combinations('ABCD', 2)) == [
    ('A', 'B'),
    ('A', 'C'),
    ('A', 'D'),
    ('B', 'C'),
    ('B', 'D'),
    ('C', 'D'),
]
assert list(itertools.combinations(range(4), 3)) == [(0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3)]
assert list(itertools.combinations([1, 2, 3], 3)) == [(1, 2, 3)]
assert list(itertools.combinations([1, 2, 3], 1)) == [(1,), (2,), (3,)]
# `r == 0` is the single empty combination, even of nothing.
assert list(itertools.combinations([1, 2], 0)) == [()]
assert list(itertools.combinations([], 0)) == [()]
# `r` past the pool yields nothing at all.
assert list(itertools.combinations([1, 2, 3], 4)) == []
assert list(itertools.combinations([], 1)) == []
# Order follows the input, not sorted order, and repeats are distinct items.
assert list(itertools.combinations([3, 1, 2], 2)) == [(3, 1), (3, 2), (1, 2)]
assert list(itertools.combinations('AAB', 2)) == [('A', 'A'), ('A', 'B'), ('A', 'B')]
# Both parameters are accepted by keyword, and `r` goes through `__index__`.
assert list(itertools.combinations(iterable=[1, 2], r=1)) == [(1,), (2,)]
assert list(itertools.combinations([1, 2], r=2)) == [(1, 2)]
assert list(itertools.combinations([1, 2], True)) == [(1,), (2,)]


class Index:
    def __index__(self):
        return 2


assert list(itertools.combinations([1, 2, 3], Index())) == [(1, 2), (1, 3), (2, 3)]
assert type(next(itertools.combinations([1], 1))) is tuple

# The input is consumed at construction, so a one-shot source is spent before
# the first `next`, and the yielded tuples hold the same objects.
source = iter([[1], [2], [3]])
combos = itertools.combinations(source, 2)
assert list(source) == []
first = next(combos)
assert first == ([1], [2])
assert first[0] is next(combos)[0]

# Partially consuming then draining picks up where `next` left off, and a spent
# iterator stays spent.
partial = itertools.combinations(range(4), 2)
assert next(partial) == (0, 1)
assert next(partial) == (0, 2)
assert list(partial) == [(0, 3), (1, 2), (1, 3), (2, 3)]
assert list(partial) == []

# === combinations errors ===
try:
    itertools.combinations()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "combinations() missing required argument 'iterable' (pos 1)"

try:
    itertools.combinations([1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "combinations() missing required argument 'r' (pos 2)"

try:
    itertools.combinations([1], 1, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'combinations() takes at most 2 arguments (3 given)'

# Arity counts positionals and keywords together.
try:
    itertools.combinations([1, 2, 3], 2, r=2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'combinations() takes at most 2 arguments (3 given)'

try:
    itertools.combinations([1], 'a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

try:
    itertools.combinations([1], 1.0)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"

try:
    itertools.combinations([1], -1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'r must be non-negative'

try:
    itertools.combinations([1], 2**63)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

try:
    itertools.combinations(5, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# `r`'s type is checked before the iterable is collected, but its sign after.
try:
    itertools.combinations(5, 'a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

try:
    itertools.combinations(5, -1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# === combinations_with_replacement ===
assert list(itertools.combinations_with_replacement([1, 2], 2)) == [(1, 1), (1, 2), (2, 2)]
assert list(itertools.combinations_with_replacement('ABC', 2)) == [
    ('A', 'A'),
    ('A', 'B'),
    ('A', 'C'),
    ('B', 'B'),
    ('B', 'C'),
    ('C', 'C'),
]
# `r` may exceed the pool, since items repeat.
assert list(itertools.combinations_with_replacement([1, 2], 3)) == [(1, 1, 1), (1, 1, 2), (1, 2, 2), (2, 2, 2)]
assert list(itertools.combinations_with_replacement([7], 4)) == [(7, 7, 7, 7)]
assert list(itertools.combinations_with_replacement([1, 2], 1)) == [(1,), (2,)]
assert list(itertools.combinations_with_replacement([1, 2], 0)) == [()]
assert list(itertools.combinations_with_replacement([], 0)) == [()]
# Only an empty pool with `r > 0` has nothing to yield.
assert list(itertools.combinations_with_replacement([], 1)) == []
assert list(itertools.combinations_with_replacement(iterable='ab', r=2)) == [('a', 'a'), ('a', 'b'), ('b', 'b')]

# === combinations_with_replacement errors ===
try:
    itertools.combinations_with_replacement()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "combinations_with_replacement() missing required argument 'iterable' (pos 1)"

try:
    itertools.combinations_with_replacement([1], 1, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'combinations_with_replacement() takes at most 2 arguments (3 given)'

try:
    itertools.combinations_with_replacement([1], -1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'r must be non-negative'

try:
    itertools.combinations_with_replacement([1], 'x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

# `r` is unbounded by the pool here, so one whose index vector no allocation
# could address raises rather than being attempted.
try:
    itertools.combinations_with_replacement('a', 2**62)
    assert False, 'expected MemoryError'
except MemoryError as exc:
    assert str(exc) == ''

# === permutations ===
assert list(itertools.permutations([1, 2, 3])) == [
    (1, 2, 3),
    (1, 3, 2),
    (2, 1, 3),
    (2, 3, 1),
    (3, 1, 2),
    (3, 2, 1),
]
assert list(itertools.permutations([1, 2, 3], 2)) == [(1, 2), (1, 3), (2, 1), (2, 3), (3, 1), (3, 2)]
assert list(itertools.permutations('AB')) == [('A', 'B'), ('B', 'A')]
assert list(itertools.permutations(range(3), 1)) == [(0,), (1,), (2,)]
assert list(itertools.permutations([1])) == [(1,)]
# `r == 0` is the single empty permutation; `r` past the pool is nothing.
assert list(itertools.permutations([1, 2, 3], 0)) == [()]
assert list(itertools.permutations([], 0)) == [()]
assert list(itertools.permutations([])) == [()]
assert list(itertools.permutations([1, 2, 3], 4)) == []
assert list(itertools.permutations([], 1)) == []
# Repeats are distinct items, so they produce repeated tuples.
assert list(itertools.permutations('AA')) == [('A', 'A'), ('A', 'A')]
# `None` means the whole pool; a `bool` counts as an `int`.
assert list(itertools.permutations([1, 2], None)) == [(1, 2), (2, 1)]
assert list(itertools.permutations([1, 2], r=None)) == [(1, 2), (2, 1)]
assert list(itertools.permutations(iterable=[1, 2], r=1)) == [(1,), (2,)]
assert list(itertools.permutations([1, 2], True)) == [(1,), (2,)]
assert len(list(itertools.permutations(range(5)))) == 120
assert len(list(itertools.permutations(range(6), 3))) == 120

# Partially consuming then draining picks up where `next` left off.
partial = itertools.permutations(range(3))
assert next(partial) == (0, 1, 2)
assert list(partial) == [(0, 2, 1), (1, 0, 2), (1, 2, 0), (2, 0, 1), (2, 1, 0)]
assert list(partial) == []

# === permutations errors ===
try:
    itertools.permutations()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "permutations() missing required argument 'iterable' (pos 1)"

try:
    itertools.permutations([1], 1, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'permutations() takes at most 2 arguments (3 given)'

# `r` must be a real `int`: neither a float nor an `__index__` object passes.
for bad_r in ('x', 1.0, Index()):
    try:
        itertools.permutations([1, 2], bad_r)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'Expected int as r'

try:
    itertools.permutations([1, 2], -1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'r must be non-negative'

try:
    itertools.permutations([1], 2**63)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

# The pool is collected before `r` is looked at.
try:
    itertools.permutations(5, 'x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# === product ===
assert list(itertools.product([1, 2], 'ab')) == [(1, 'a'), (1, 'b'), (2, 'a'), (2, 'b')]
assert list(itertools.product('AB', 'xy', [0])) == [('A', 'x', 0), ('A', 'y', 0), ('B', 'x', 0), ('B', 'y', 0)]
assert list(itertools.product([1, 2, 3])) == [(1,), (2,), (3,)]
# `repeat` multiplies the arguments out, in order.
assert list(itertools.product([0, 1], repeat=2)) == [(0, 0), (0, 1), (1, 0), (1, 1)]
assert list(itertools.product('ab', [1], repeat=2)) == [
    ('a', 1, 'a', 1),
    ('a', 1, 'b', 1),
    ('b', 1, 'a', 1),
    ('b', 1, 'b', 1),
]
assert len(list(itertools.product(range(3), repeat=3))) == 27
# No pools at all is the single empty tuple, however that comes about.
assert list(itertools.product()) == [()]
assert list(itertools.product(repeat=3)) == [()]
assert list(itertools.product([1, 2], repeat=0)) == [()]
# An empty pool empties the whole product.
assert list(itertools.product([], [1])) == []
assert list(itertools.product([1], [])) == []
assert list(itertools.product([], repeat=2)) == []
# `repeat=0` never touches the arguments, so a non-iterable passes.
assert list(itertools.product(5, repeat=0)) == [()]
assert list(itertools.product([1, 2], repeat=True)) == [(1,), (2,)]
assert list(itertools.product([1, 2], repeat=Index())) == [(1, 1), (1, 2), (2, 1), (2, 2)]
assert type(next(itertools.product([1]))) is tuple

# Every argument is consumed at construction, so the same iterator passed twice
# contributes only to the first slot.
shared = iter([1, 2])
assert list(itertools.product(shared, shared)) == []
source = iter('ab')
assert list(itertools.product(source, repeat=2)) == [('a', 'a'), ('a', 'b'), ('b', 'a'), ('b', 'b')]

# Partially consuming then draining picks up where `next` left off.
partial = itertools.product([0, 1], [0, 1])
assert next(partial) == (0, 0)
assert list(partial) == [(0, 1), (1, 0), (1, 1)]
assert list(partial) == []

# === product errors ===
try:
    itertools.product([1], repeat=-1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'repeat argument cannot be negative'

try:
    itertools.product([1], repeat='x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

try:
    itertools.product([1], repeat=1.0)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"

try:
    itertools.product([1], repeat=2**63)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

try:
    itertools.product([1], foo=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "product() got an unexpected keyword argument 'foo'"

# A `repeat` that puts the index vector past what a machine integer can address
# is rejected before the arguments are even looked at, so a non-iterable
# argument alongside it goes unreported.
for bad_product in (
    lambda: itertools.product([1], [2], [3], repeat=2**62),
    lambda: itertools.product('ab', repeat=2**62),
    lambda: itertools.product(5, repeat=2**62),
):
    try:
        bad_product()
        assert False, 'expected OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'repeat argument too large'

# The negative check comes first, though.
try:
    itertools.product(5, repeat=-1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'repeat argument cannot be negative'

# With no iterables there is no index vector to size, so the same `repeat` is
# fine and yields the one empty tuple.
assert list(itertools.product(repeat=2**62)) == [()]

# An empty pool empties the product, so no index vector is built however large
# `repeat` is. Kept modest here because CPython does size its own vector from
# `repeat` before noticing the empty pool; the memory-limit side of this is
# `empty_product_pool_is_not_preflighted` in the subprocess tests.
assert list(itertools.product([], repeat=10**5)) == []
assert list(itertools.product([1], [], repeat=10**5)) == []

try:
    itertools.product([1], 5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# `repeat` is checked before any argument is collected.
try:
    itertools.product(5, repeat=-1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'repeat argument cannot be negative'

# === Iterator protocol and types ===
for adaptor in (
    itertools.combinations([1], 1),
    itertools.combinations_with_replacement([1], 1),
    itertools.permutations([1]),
    itertools.product([1]),
):
    assert iter(adaptor) is adaptor
    assert next(adaptor) == (1,)
    try:
        next(adaptor)
        assert False, 'expected StopIteration'
    except StopIteration:
        pass

assert str(type(itertools.combinations([], 0))) == "<class 'itertools.combinations'>"
assert type(itertools.combinations([], 0)).__name__ == 'combinations'
assert str(type(itertools.combinations_with_replacement([], 0))) == "<class 'itertools.combinations_with_replacement'>"
assert type(itertools.combinations_with_replacement([], 0)).__name__ == 'combinations_with_replacement'
assert str(type(itertools.permutations([]))) == "<class 'itertools.permutations'>"
assert type(itertools.permutations([])).__name__ == 'permutations'
assert str(type(itertools.product())) == "<class 'itertools.product'>"
assert type(itertools.product()).__name__ == 'product'
product_repr = itertools.product()
product_repr_text = repr(product_repr)
assert product_repr_text.startswith('<itertools.product object at 0x')
assert int(product_repr_text.rsplit(' at ', 1)[1][:-1], 16) == id(product_repr)


# === Sources ===
# A user-defined iterator is collected through the VM like any other.
class UpTo:
    def __init__(self, limit):
        self.limit = limit
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > self.limit:
            raise StopIteration
        return self.n


assert list(itertools.combinations(UpTo(3), 2)) == [(1, 2), (1, 3), (2, 3)]
assert list(itertools.permutations(UpTo(2))) == [(1, 2), (2, 1)]
assert list(itertools.product(UpTo(2), UpTo(2))) == [(1, 1), (1, 2), (2, 1), (2, 2)]


# A source that raises does so at construction, since that is when it is read.
class Boom:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('boom')


for build in (
    lambda: itertools.combinations(Boom(), 1),
    lambda: itertools.combinations_with_replacement(Boom(), 1),
    lambda: itertools.permutations(Boom()),
    lambda: itertools.product(Boom()),
):
    try:
        build()
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'boom'

# === Composition ===
assert list(itertools.islice(itertools.permutations(range(10)), 2)) == [
    (0, 1, 2, 3, 4, 5, 6, 7, 8, 9),
    (0, 1, 2, 3, 4, 5, 6, 7, 9, 8),
]
assert list(itertools.combinations(itertools.islice(itertools.count(), 3), 2)) == [(0, 1), (0, 2), (1, 2)]
assert list(itertools.product(itertools.repeat('x', 2), 'ab')) == [('x', 'a'), ('x', 'b'), ('x', 'a'), ('x', 'b')]
assert list(itertools.starmap(lambda a, b: a * b, itertools.combinations([2, 3, 4], 2))) == [6, 8, 12]
assert sum(a * b for a, b in itertools.product([1, 2], [3, 4])) == 21
assert sorted(set(itertools.combinations_with_replacement('ba', 2))) == [('a', 'a'), ('b', 'a'), ('b', 'b')]
assert dict(itertools.product('ab', [0])) == {'a': 0, 'b': 0}
assert (1, 3) in itertools.combinations([1, 2, 3], 2)
assert (3, 1) not in itertools.combinations([1, 2, 3], 2)

# Tuple unpacking in a for loop.
total = 0
for a, b in itertools.permutations([1, 2, 3], 2):
    total += a * 10 + b
assert total == 132
