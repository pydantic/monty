# The `itertools` adaptors that wrap a source iterator, as opposed to the
# self-contained infinite ones in `itertools__count_repeat.py`.
import itertools

# === pairwise ===
assert list(itertools.pairwise([1, 2, 3, 4])) == [(1, 2), (2, 3), (3, 4)]
assert list(itertools.pairwise('abc')) == [('a', 'b'), ('b', 'c')]
assert list(itertools.pairwise(range(4))) == [(0, 1), (1, 2), (2, 3)]
# Fewer than two items pairs nothing.
assert list(itertools.pairwise([1])) == []
assert list(itertools.pairwise([])) == []

# Each item is reused as the left half of the following pair, so the same
# object appears twice rather than being re-fetched from the source.
shared = [0]
pairs = list(itertools.pairwise([shared, shared, shared]))
assert pairs == [([0], [0]), ([0], [0])]
assert pairs[0][1] is pairs[1][0]

# Partially consuming then draining picks up where `next` left off.
partial = itertools.pairwise([1, 2, 3, 4])
assert next(partial) == (1, 2)
assert list(partial) == [(2, 3), (3, 4)]
# A spent adaptor stays spent.
assert list(partial) == []

# Consuming an exhausted source again yields nothing rather than resuming.
spent = itertools.pairwise([1, 2])
assert list(spent) == [(1, 2)]
assert list(spent) == []

# It is its own iterator, as CPython's adaptors all are.
p = itertools.pairwise([1, 2])
assert iter(p) is p

# `str(type(...))` matches CPython; `__name__` is the dotted form Monty uses
# for every dotted `tp_name` (see limitations/itertools.md).
assert str(type(itertools.pairwise([]))) == "<class 'itertools.pairwise'>"

# === pairwise errors ===
try:
    itertools.pairwise()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'pairwise expected 1 argument, got 0'

try:
    itertools.pairwise([1], [2])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'pairwise expected 1 argument, got 2'

try:
    itertools.pairwise(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

try:
    itertools.pairwise(iterable=[1, 2])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'pairwise() takes no keyword arguments'

# === compress ===
assert list(itertools.compress('ABCDEF', [1, 0, 1, 0, 1, 1])) == ['A', 'C', 'E', 'F']
# Stops with the shorter side, whichever it is.
assert list(itertools.compress('ABC', [1, 1, 1, 1, 1])) == ['A', 'B', 'C']
assert list(itertools.compress('ABCDEF', [1, 1])) == ['A', 'B']
# Selection is by truthiness, not equality with True.
assert list(itertools.compress([1, 2, 3], ['x', '', None])) == [1]
assert list(itertools.compress([1, 2, 3], [[], [0], {}])) == [2]
assert list(itertools.compress([], [])) == []
# Both parameters are also accepted by keyword.
assert list(itertools.compress(data='ABC', selectors=[0, 1, 1])) == ['B', 'C']

# === compress errors ===
try:
    itertools.compress()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "compress() missing required argument 'data' (pos 1)"

try:
    itertools.compress([1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "compress() missing required argument 'selectors' (pos 2)"

# Arity counts positionals and keywords together.
try:
    itertools.compress([1], [1], data=[1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'compress() takes at most 2 arguments (3 given)'

try:
    itertools.compress(5, [1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# === islice ===
assert list(itertools.islice('ABCDEFG', 2)) == ['A', 'B']
assert list(itertools.islice('ABCDEFG', 2, 4)) == ['C', 'D']
assert list(itertools.islice('ABCDEFG', 2, None, 2)) == ['C', 'E', 'G']
assert list(itertools.islice('ABCDEFG', 0, None, 3)) == ['A', 'D', 'G']
# A `None` stop means "to the end"; a `None` start or step means 0 and 1.
assert list(itertools.islice('ABCDEFG', None)) == ['A', 'B', 'C', 'D', 'E', 'F', 'G']
assert list(itertools.islice('ABC', None, 2)) == ['A', 'B']
assert list(itertools.islice('ABC', 0, 2, None)) == ['A', 'B']
# A stop past the end just runs out.
assert list(itertools.islice('AB', 10)) == ['A', 'B']
assert list(itertools.islice('ABC', 5, 10)) == []
assert list(itertools.islice([], 3)) == []

# Only the items needed are consumed, so the source can be used afterwards.
source = iter('ABCDEFG')
assert list(itertools.islice(source, 2)) == ['A', 'B']
assert list(source) == ['C', 'D', 'E', 'F', 'G']

# === islice errors ===
try:
    itertools.islice([1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'islice expected at least 2 arguments, got 1'

try:
    itertools.islice([1], 1, 2, 3, 4)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'islice expected at most 4 arguments, got 5'

# The two-argument form names the stop argument...
try:
    itertools.islice([1], -1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize.'

# ...while three or more arguments stop distinguishing which was at fault.
try:
    itertools.islice([1], -1, 2)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.'

try:
    itertools.islice([1], 0, 1, 0)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Step for islice() must be a positive integer or None.'

try:
    itertools.islice([1], stop=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'islice() takes no keyword arguments'

# === chain ===
assert list(itertools.chain([1, 2], [3], [4, 5])) == [1, 2, 3, 4, 5]
assert list(itertools.chain()) == []
assert list(itertools.chain([1, 2])) == [1, 2]
assert list(itertools.chain('ab', 'cd')) == ['a', 'b', 'c', 'd']
# Empty arguments are skipped rather than ending the chain.
assert list(itertools.chain([], [1], [], [2], [])) == [1, 2]
assert list(itertools.chain([], [])) == []
# Mixed iterable types concatenate fine.
assert list(itertools.chain(range(2), 'a', (9,))) == [0, 1, 'a', 9]

# chain resolves each argument only when it reaches it, so a bad argument
# constructs fine and raises part-way through.
lazy = itertools.chain([1], 5)
assert next(lazy) == 1
try:
    next(lazy)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

try:
    itertools.chain(x=[1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'chain() takes no keyword arguments'

# === cycle ===
assert list(itertools.islice(itertools.cycle([1, 2, 3]), 7)) == [1, 2, 3, 1, 2, 3, 1]
assert list(itertools.islice(itertools.cycle('ab'), 5)) == ['a', 'b', 'a', 'b', 'a']
assert list(itertools.islice(itertools.cycle([9]), 3)) == [9, 9, 9]
# An empty source cycles nothing rather than looping forever.
assert list(itertools.cycle([])) == []
assert list(itertools.islice(itertools.cycle([]), 5)) == []

# The saved items are the same objects on every pass, not copies.
element = [0]
repeated = list(itertools.islice(itertools.cycle([element]), 3))
assert repeated == [[0], [0], [0]]
assert repeated[0] is repeated[1] is repeated[2]

# Cycling consumes the source lazily, one item per step on the first pass.
drained = iter([1, 2, 3])
partial_cycle = itertools.cycle(drained)
assert next(partial_cycle) == 1
assert next(partial_cycle) == 2

# === cycle errors ===
try:
    itertools.cycle()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'cycle expected 1 argument, got 0'

try:
    itertools.cycle([1], [2])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'cycle expected 1 argument, got 2'

# Unlike chain, cycle resolves its argument eagerly.
try:
    itertools.cycle(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

try:
    itertools.cycle(iterable=[1])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'cycle() takes no keyword arguments'

# === Iterator protocol ===
# Every adaptor is its own iterator, and exhaustion raises StopIteration rather
# than returning a sentinel.
for spent in (
    itertools.pairwise([1, 2]),
    itertools.compress([1], [1]),
    itertools.islice([1], 1),
    itertools.chain([1]),
    itertools.cycle([1, 2]),
):
    assert iter(spent) is spent

for exhausted in (
    itertools.pairwise([1, 2]),
    itertools.compress([1], [1]),
    itertools.islice([1], 1),
    itertools.chain([1]),
):
    next(exhausted)
    try:
        next(exhausted)
        assert False, 'expected StopIteration'
    except StopIteration:
        pass

# An empty source stops immediately rather than yielding once.
try:
    next(itertools.cycle([]))
    assert False, 'expected StopIteration'
except StopIteration:
    pass


# === User-defined iterators as sources ===
# The adaptors drive `__next__` back through the VM, so a Python-level source
# exercises a different path from the built-in iterators above.
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


assert list(itertools.pairwise(UpTo(3))) == [(1, 2), (2, 3)]
assert list(itertools.compress(UpTo(3), [1, 0, 1])) == [1, 3]
assert list(itertools.islice(UpTo(5), 1, 4)) == [2, 3, 4]
assert list(itertools.chain(UpTo(2), UpTo(2))) == [1, 2, 1, 2]
assert list(itertools.islice(itertools.cycle(UpTo(2)), 5)) == [1, 2, 1, 2, 1]


# === Exceptions from a source propagate ===
class Boom:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('boom')


for failing in (
    itertools.pairwise(Boom()),
    itertools.compress(Boom(), [1]),
    itertools.islice(Boom(), 1),
    itertools.chain(Boom()),
    itertools.cycle(Boom()),
):
    try:
        next(failing)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'boom'


# === Composition ===
# Adaptors feed each other, including bounding an infinite source.
assert list(itertools.pairwise(itertools.islice(itertools.count(), 4))) == [(0, 1), (1, 2), (2, 3)]
assert list(itertools.islice(itertools.chain(itertools.repeat(1, 2), [2]), 2)) == [1, 1]
assert list(itertools.compress(itertools.chain('ab', 'cd'), itertools.cycle([1, 0]))) == ['a', 'c']
assert list(itertools.islice(itertools.cycle(itertools.islice('abcdef', 2)), 5)) == ['a', 'b', 'a', 'b', 'a']
assert list(itertools.chain(itertools.pairwise([1, 2, 3]), [(9, 9)])) == [(1, 2), (2, 3), (9, 9)]

# === Collection builders and iteration syntax ===
assert tuple(itertools.pairwise([1, 2, 3])) == ((1, 2), (2, 3))
assert sorted(itertools.chain([3, 1], [2])) == [1, 2, 3]
assert set(itertools.compress('aab', [1, 1, 1])) == {'a', 'b'}
assert sorted(set(itertools.islice(itertools.cycle('ab'), 5))) == ['a', 'b']

# Tuple unpacking in a for loop over a pairwise.
total = 0
for left, right in itertools.pairwise([1, 2, 3]):
    total += left * right
assert total == 8

# Membership consumes the adaptor until it matches.
assert 3 in itertools.chain([1, 2], [3])
assert 'z' not in itertools.compress('abc', [1, 1, 1])
