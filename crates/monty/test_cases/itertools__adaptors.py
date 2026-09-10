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

# The dotted `tp_name` shows in `str(type(...))`, the bare one in `__name__`,
# as CPython does it.
assert str(type(itertools.pairwise([]))) == "<class 'itertools.pairwise'>"
assert type(itertools.pairwise([])).__name__ == 'pairwise'
pairwise_repr = itertools.pairwise([])
pairwise_repr_text = repr(pairwise_repr)
assert pairwise_repr_text.startswith('<itertools.pairwise object at 0x')
assert int(pairwise_repr_text.rsplit(' at ', 1)[1][:-1], 16) == id(pairwise_repr)

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

# An argument that fails `iter()` ends the chain: CPython drops the source, so
# the arguments after the bad one are never reached and the chain stays spent.
spent = itertools.chain([1], 5, [2, 3])
assert next(spent) == 1
try:
    next(spent)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
for _ in range(2):
    try:
        next(spent)
        assert False, 'expected the chain to be spent'
    except StopIteration:
        pass
assert list(spent) == []

# the same when the very first argument is the one that fails
first_bad = itertools.chain(5, [2, 3])
try:
    next(first_bad)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
assert list(first_bad) == []


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


# starmap has no spent flag either, so a source that stops and then yields again
# is re-driven rather than treated as finished.
class StutteringPairs:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 2:
            raise StopIteration
        return (self.calls, 2)


starred = itertools.starmap(pow, StutteringPairs())
assert next(starred) == 1
try:
    next(starred)
    assert False, 'expected StopIteration'
except StopIteration:
    pass
assert next(starred) == 9

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

# For chain the error is not the end of it: only a source that fails `iter()`
# ends the chain, so a source that fails `__next__` stays in place and CPython
# hands back the same error on the next call rather than moving to `[9]`.
still_live = itertools.chain(Boom(), [9])
for _ in range(2):
    try:
        next(still_live)
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


# === takewhile ===
assert list(itertools.takewhile(lambda x: x < 3, [1, 2, 3, 4, 1])) == [1, 2]
assert list(itertools.takewhile(lambda x: x < 3, [])) == []
assert list(itertools.takewhile(lambda x: False, [1, 2])) == []
assert list(itertools.takewhile(lambda x: True, 'ab')) == ['a', 'b']
# A None predicate is only reached when there is an item to test.
assert list(itertools.takewhile(None, [])) == []

# The predicate stops being called at the first rejection, and the adaptor
# stays spent afterwards.
seen = []


def under_two(x):
    seen.append(x)
    return x < 2


spent = itertools.takewhile(under_two, [1, 2, 3])
assert list(spent) == [1]
assert seen == [1, 2]
assert list(spent) == []
assert seen == [1, 2]


# Only a rejected item latches the adaptor. A source that raises StopIteration
# and then yields again is re-driven, as CPython's takewhile does — it is
# `pairwise`/`islice` that release their source when it runs out, not this one.
class Stuttering:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 2:
            raise StopIteration
        return self.calls


stutter = itertools.takewhile(lambda x: x < 4, Stuttering())
assert next(stutter) == 1
try:
    next(stutter)
    assert False, 'expected StopIteration'
except StopIteration:
    pass
assert next(stutter) == 3
# The same source behaviour through the other two, which never latched. Each is
# driven *past* the StopIteration, since stopping at the first item would pass
# whether or not the adaptor wrongly treated exhaustion as terminal.
for adaptor in (
    itertools.dropwhile(lambda x: False, Stuttering()),
    itertools.filterfalse(lambda x: False, Stuttering()),
):
    assert next(adaptor) == 1
    try:
        next(adaptor)
        assert False, 'expected StopIteration'
    except StopIteration:
        pass
    assert next(adaptor) == 3


# Latching stops the source being touched, not only the predicate being called:
# a second drain must not reach it. `Counting` reports how often it was asked.
class Counting:
    def __init__(self, items):
        self.items = list(items)
        self.reads = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.reads += 1
        if not self.items:
            raise StopIteration
        return self.items.pop(0)


counted = Counting([1, 5, 2])
latched = itertools.takewhile(lambda x: x < 3, counted)
assert list(latched) == [1]
assert counted.reads == 2
assert list(latched) == []
assert counted.reads == 2

# === dropwhile ===
assert list(itertools.dropwhile(lambda x: x < 3, [1, 2, 3, 4, 1])) == [3, 4, 1]
assert list(itertools.dropwhile(lambda x: x < 3, [])) == []
assert list(itertools.dropwhile(lambda x: True, [1, 2])) == []
assert list(itertools.dropwhile(lambda x: False, [1, 2])) == [1, 2]

# Once the predicate has failed it is never consulted again, so later items
# are yielded even when they would have satisfied it.
dropped = []


def small(x):
    dropped.append(x)
    return x < 2


assert list(itertools.dropwhile(small, [1, 2, 3, 0])) == [2, 3, 0]
assert dropped == [1, 2]

# === filterfalse ===
assert list(itertools.filterfalse(lambda x: x % 2, range(6))) == [0, 2, 4]
assert list(itertools.filterfalse(lambda x: True, [1, 2])) == []
assert list(itertools.filterfalse(lambda x: False, [1, 2])) == [1, 2]
assert list(itertools.filterfalse(lambda x: x % 2, [])) == []
# A None predicate selects the truth test, keeping the falsy items.
assert list(itertools.filterfalse(None, [0, 1, '', 'a', [], None])) == [0, '', [], None]
assert list(itertools.filterfalse(None, [1, 'a', [2]])) == []

# === starmap ===
assert list(itertools.starmap(pow, [(2, 5), (3, 2)])) == [32, 9]
assert list(itertools.starmap(lambda a, b: a + b, ['ab', 'cd'])) == ['ab', 'cd']
assert list(itertools.starmap(max, [[1, 5, 3]])) == [5]
assert list(itertools.starmap(pow, [])) == []
# Items are spread, so a single-element item calls a single-argument function.
assert list(itertools.starmap(abs, [(-2,), (3,)])) == [2, 3]

# === Iterator protocol ===
for adaptor in (
    itertools.takewhile(bool, [1]),
    itertools.dropwhile(bool, [1]),
    itertools.filterfalse(bool, [0]),
    itertools.starmap(pow, [(2, 2)]),
):
    assert iter(adaptor) is adaptor

exhausted = itertools.takewhile(lambda x: True, [1])
assert next(exhausted) == 1
try:
    next(exhausted)
    assert False, 'expected StopIteration'
except StopIteration:
    pass


# Returning one from a user `__iter__` works, which needs the adaptor to count
# as a concrete iterator type and not just as something iterable.
class Wrapped:
    def __init__(self, adaptor):
        self.adaptor = adaptor

    def __iter__(self):
        return self.adaptor


assert list(Wrapped(itertools.takewhile(lambda x: x < 3, [1, 2, 3]))) == [1, 2]
assert list(Wrapped(itertools.dropwhile(lambda x: x < 3, [1, 2, 3]))) == [3]
assert list(Wrapped(itertools.filterfalse(None, [0, 1]))) == [0]
assert list(Wrapped(itertools.starmap(pow, [(2, 3)]))) == [8]


# === Signature errors ===
for name, builder in (
    ('takewhile', itertools.takewhile),
    ('dropwhile', itertools.dropwhile),
    ('filterfalse', itertools.filterfalse),
    ('starmap', itertools.starmap),
):
    try:
        builder()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == name + ' expected 2 arguments, got 0'
    try:
        builder(bool)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == name + ' expected 2 arguments, got 1'
    try:
        builder(bool, [], [])
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == name + ' expected 2 arguments, got 3'
    try:
        builder(bool, iterable=[])
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == name + '() takes no keyword arguments'
    # The iterable is resolved eagerly, so a non-iterable raises up front.
    try:
        builder(bool, 5)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "'int' object is not iterable"

# A None predicate is a call-time failure, not a construction-time one.
for adaptor in (itertools.takewhile(None, [1]), itertools.dropwhile(None, [1])):
    try:
        next(adaptor)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not callable"

# starmap needs each item to be iterable, discovered as it reaches them.
bad_items = itertools.starmap(pow, [5])
try:
    next(bad_items)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"


# === Exceptions propagate out of the callable ===
def explode(x):
    raise ValueError('bang')


for adaptor in (
    itertools.takewhile(explode, [1]),
    itertools.dropwhile(explode, [1]),
    itertools.filterfalse(explode, [1]),
    itertools.starmap(explode, [(1,)]),
):
    try:
        next(adaptor)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'bang'

# === Composition with the source-wrapping adaptors ===
assert list(itertools.takewhile(lambda x: x < 4, itertools.count())) == [0, 1, 2, 3]
assert list(itertools.islice(itertools.dropwhile(lambda x: x < 3, itertools.count()), 3)) == [3, 4, 5]
assert list(itertools.filterfalse(None, itertools.islice(itertools.cycle([0, 1]), 4))) == [0, 0]
assert list(itertools.starmap(pow, itertools.pairwise([2, 3, 4]))) == [8, 81]
assert sorted(itertools.filterfalse(lambda x: x > 1, itertools.chain([0, 1], [2]))) == [0, 1]

# Every argument-count shape, since each is packed differently internally.
assert list(itertools.starmap(lambda: 7, [()])) == [7]
assert list(itertools.starmap(lambda a, b, c: a + b + c, [(1, 2, 3)])) == [6]
assert list(itertools.starmap(lambda *a: len(a), [(1, 2, 3, 4)])) == [4]

# === accumulate ===
assert list(itertools.accumulate([1, 2, 3, 4])) == [1, 3, 6, 10]
assert list(itertools.accumulate([1, 2, 3, 4], lambda a, b: a * b)) == [1, 2, 6, 24]
assert list(itertools.accumulate([])) == []
assert list(itertools.accumulate([5])) == [5]
assert list(itertools.accumulate(['a', 'b', 'c'])) == ['a', 'ab', 'abc']
# The first item is yielded untouched, never passed through the callable.
assert list(itertools.accumulate([7], lambda a, b: 0)) == [7]
# `initial` is yielded before the source is touched, so an empty source still
# produces one value.
assert list(itertools.accumulate([1, 2, 3], initial=10)) == [10, 11, 13, 16]
assert list(itertools.accumulate([], initial=10)) == [10]
# An explicit `None` is no initial at all, and no callable means `+`.
assert list(itertools.accumulate([1, 2, 3], initial=None)) == [1, 3, 6]
assert list(itertools.accumulate([1, 2], None)) == [1, 3]
assert list(itertools.accumulate([1, 2], func=None)) == [1, 3]
assert list(itertools.accumulate(iterable=[1, 2])) == [1, 3]

# === batched ===
assert list(itertools.batched('ABCDEFG', 3)) == [('A', 'B', 'C'), ('D', 'E', 'F'), ('G',)]
assert list(itertools.batched([1, 2, 3, 4], 2)) == [(1, 2), (3, 4)]
assert list(itertools.batched([], 3)) == []
assert list(itertools.batched([1, 2], 5)) == [(1, 2)]
assert list(itertools.batched([1, 2, 3], n=3)) == [(1, 2, 3)]
assert list(itertools.batched('ABCD', 2, strict=True)) == [('A', 'B'), ('C', 'D')]
assert type(next(itertools.batched([1], 1))) is tuple

# === zip_longest ===
assert list(itertools.zip_longest([1, 2, 3], 'ab')) == [(1, 'a'), (2, 'b'), (3, None)]
assert list(itertools.zip_longest([1, 2, 3], 'ab', fillvalue='-')) == [(1, 'a'), (2, 'b'), (3, '-')]
assert list(itertools.zip_longest()) == []
assert list(itertools.zip_longest([1, 2])) == [(1,), (2,)]
assert list(itertools.zip_longest([], [])) == []
assert list(itertools.zip_longest([1], [2, 3], [4, 5, 6])) == [(1, 2, 4), (None, 3, 5), (None, None, 6)]
assert type(next(itertools.zip_longest([1]))) is tuple
# Exhausted once, exhausted for good.
spent_zip = itertools.zip_longest([1], [2, 3])
assert list(spent_zip) == [(1, 2), (None, 3)]
assert list(spent_zip) == []


# A source that steps the same `zip_longest` from inside its own `__next__`
# drains the other slots before the outer round reaches them, so the outer round
# pads them. What the adaptor does AFTER that row diverges — see
# `limitations/itertools.md`.
class Reentrant:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 1:
            try:
                next(reentrant_zip)
                assert False, 'expected the nested round to exhaust every slot'
            except StopIteration:
                pass
            return 'a'
        raise StopIteration


reentrant_zip = itertools.zip_longest(Reentrant(), iter([]), iter([]))
assert next(reentrant_zip) == ('a', None, None)

# === the new adaptors against a stuttering source ===
# `batched` clears its source only on an EMPTY batch, so a short batch caused
# by a transient StopIteration is yielded and the source driven again.
# `Stuttering` never really ends, so it is stepped rather than drained.
stutter_batched = itertools.batched(Stuttering(), 3)
assert next(stutter_batched) == (1,)
assert next(stutter_batched) == (3, 4, 5)
# `accumulate` keeps its running total across the same stutter.
stutter_acc = itertools.accumulate(Stuttering())
assert next(stutter_acc) == 1
try:
    next(stutter_acc)
    assert False, 'expected StopIteration'
except StopIteration:
    pass
assert next(stutter_acc) == 4

# === errors shared with CPython ===
# Arity and clinic wording diverge (see limitations/itertools.md), so only the
# messages that match both engines are pinned here.
try:
    itertools.accumulate(5)
    assert False, 'expected accumulate to reject a non-iterable'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

try:
    list(itertools.accumulate([1, 2], 5))
    assert False, 'expected accumulate to reject a non-callable func'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"

try:
    list(itertools.accumulate([1, 'a']))
    assert False, 'expected accumulate to reject mixed operands'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'int' and 'str'"

try:
    itertools.accumulate([1], bogus=1)
    assert False, 'expected accumulate to reject an unknown keyword'
except TypeError as exc:
    assert str(exc) == "accumulate() got an unexpected keyword argument 'bogus'"

# Clinic shares its arity wording with the named C family: "at most" when a
# positional has a default, "exactly" when none does, and a total count once
# keywords push the overflow past every slot.
try:
    itertools.accumulate([1], None, 9)
    assert False, 'expected accumulate to reject a third positional'
except TypeError as exc:
    assert str(exc) == 'accumulate() takes at most 2 positional arguments (3 given)'

try:
    itertools.accumulate([1], None, 9, initial=1)
    assert False, 'expected accumulate to reject a third positional with a keyword'
except TypeError as exc:
    assert str(exc) == 'accumulate() takes at most 3 arguments (4 given)'

try:
    itertools.batched([1], 2, 3)
    assert False, 'expected batched to reject a third positional'
except TypeError as exc:
    assert str(exc) == 'batched() takes exactly 2 positional arguments (3 given)'

try:
    itertools.accumulate()
    assert False, 'expected accumulate to reject a missing iterable'
except TypeError as exc:
    assert str(exc) == "accumulate() missing required argument 'iterable' (pos 1)"

try:
    itertools.batched([1])
    assert False, 'expected batched to reject a missing n'
except TypeError as exc:
    assert str(exc) == "batched() missing required argument 'n' (pos 2)"

try:
    itertools.accumulate([1], None, func=abs)
    assert False, 'expected accumulate to reject a duplicated func'
except TypeError as exc:
    assert str(exc) == "argument for accumulate() given by name ('func') and position (2)"

for bad_n in (0, -1):
    try:
        itertools.batched([1], bad_n)
        assert False, 'expected batched to reject n below one'
    except ValueError as exc:
        assert str(exc) == 'n must be at least one'

try:
    itertools.batched([1], 'x')
    assert False, 'expected batched to reject a non-integer n'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

# `n` is a `Py_ssize_t`, so one past it overflows rather than saturating. On a
# 64-bit host that ceiling is `i64`; a 32-bit one rejects from `2**31` up too.
try:
    itertools.batched([1], 2**70)
    assert False, 'expected batched to reject an n past Py_ssize_t'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

# `strict` rejects the short final batch, and latches rather than retrying.
strict_batched = itertools.batched('ABCDE', 2, strict=True)
assert next(strict_batched) == ('A', 'B')
assert next(strict_batched) == ('C', 'D')
try:
    next(strict_batched)
    assert False, 'expected batched to reject the incomplete batch'
except ValueError as exc:
    assert str(exc) == 'batched(): incomplete batch'
try:
    next(strict_batched)
    assert False, 'expected batched to be spent'
except StopIteration:
    pass

# Every `zip_longest` argument is resolved eagerly, so a later non-iterable
# raises at construction rather than on the first `next`.
try:
    itertools.zip_longest([1], 5)
    assert False, 'expected zip_longest to reject a non-iterable'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
