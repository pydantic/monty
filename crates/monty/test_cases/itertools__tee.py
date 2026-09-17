# `itertools.tee`: independent iterators over one source, which each read every
# item exactly once between them and buffer what the others have not reached.
import itertools

# === Basics ===
assert [list(x) for x in itertools.tee([1, 2, 3])] == [[1, 2, 3], [1, 2, 3]]
assert [list(x) for x in itertools.tee('ab', 3)] == [['a', 'b'], ['a', 'b'], ['a', 'b']]
assert [list(x) for x in itertools.tee([1, 2], 1)] == [[1, 2]]
assert [list(x) for x in itertools.tee([], 2)] == [[], []]
# `n` is the length of the tuple, so zero of them is an empty tuple — and with
# no consumer to read it, the argument is never even iterated.
assert itertools.tee([1], 0) == ()
assert itertools.tee(5, 0) == ()
assert type(itertools.tee([1])) is tuple
assert len(itertools.tee([1, 2], 5)) == 5

# The consumers are independent: one running ahead does not move the others.
first, second = itertools.tee([1, 2, 3])
assert next(first) == 1
assert next(first) == 2
assert next(second) == 1
assert list(first) == [3]
assert list(second) == [2, 3]

# Interleaving them reads each item from the source once.
reads = []


class Counted:
    def __init__(self, items):
        self.items = list(items)

    def __iter__(self):
        return self

    def __next__(self):
        if not self.items:
            raise StopIteration
        item = self.items.pop(0)
        reads.append(item)
        return item


left, right = itertools.tee(Counted([1, 2, 3]))
assert (next(left), next(right)) == (1, 1)
assert reads == [1]
assert (next(left), next(right)) == (2, 2)
assert reads == [1, 2]
assert list(left) == [3]
assert list(right) == [3]
assert reads == [1, 2, 3]

# The source is consumed lazily, only as far as the furthest consumer.
source = iter([1, 2, 3])
lazy, _lazy_other = itertools.tee(source)
assert next(lazy) == 1
assert list(source) == [2, 3]

# A consumer released before the other is drained takes nothing with it: the
# buffer belongs to the group, not to whichever iterator reached an item first.
discarded, kept = itertools.tee(range(3))
assert next(discarded) == 0
discarded = None
assert list(kept) == [0, 1, 2]

# Exhaustion is per consumer and sticks.
spent, other = itertools.tee([1])
assert list(spent) == [1]
assert list(spent) == []
assert list(other) == [1]


# The source is re-read rather than latched, so one that stops and later
# yields again is picked up where it left off — and both consumers see the
# same sequence.
class Stuttering:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 2 or self.calls > 4:
            raise StopIteration
        return self.calls


stutter_a, stutter_b = itertools.tee(Stuttering())
assert next(stutter_a) == 1
assert next(stutter_a, 'STOP') == 'STOP'
assert next(stutter_a) == 3
assert [next(stutter_b, 'STOP') for _ in range(4)] == [1, 3, 4, 'STOP']

# === Copying ===
# `tee` of a `_tee` copies where it stands rather than draining it, so the
# copies replay from there and advancing the original leaves them alone.
original, sibling = itertools.tee([1, 2, 3, 4])
copy_one, copy_two = itertools.tee(original, 2)
assert next(original) == 1
assert list(copy_one) == [1, 2, 3, 4]
assert list(copy_two) == [1, 2, 3, 4]
assert list(sibling) == [1, 2, 3, 4]
assert list(original) == [2, 3, 4]

# Copying part-way through picks up from the copied position.
advanced, _advanced_other = itertools.tee([1, 2, 3])
assert next(advanced) == 1
(resumed,) = itertools.tee(advanced, 1)
assert list(resumed) == [2, 3]


# The argument is resolved BEFORE it is tested for copying, so an object whose
# `__iter__` hands back a `_tee` is copied too rather than drained into a
# second buffer behind it.
wrapped, wrapped_sibling = itertools.tee([1, 2, 3, 4])
assert next(wrapped) == 1


class WrapsTee:
    def __iter__(self):
        return wrapped


wrap_one, wrap_two = itertools.tee(WrapsTee())
assert list(wrap_one) == [2, 3, 4]
assert list(wrap_two) == [2, 3, 4]
# Untouched by the copies, where draining it would have left it spent.
assert next(wrapped) == 2
assert list(wrapped_sibling) == [1, 2, 3, 4]

# === Iterator protocol and types ===
tee_iter = itertools.tee([1])[0]
assert iter(tee_iter) is tee_iter
assert next(tee_iter) == 1
try:
    next(tee_iter)
    assert False, 'expected StopIteration'
except StopIteration:
    pass

assert str(type(itertools.tee([1])[0])) == "<class 'itertools._tee'>"
assert type(itertools.tee([1])[0]).__name__ == '_tee'
tee_repr = itertools.tee([1])[0]
tee_repr_text = repr(tee_repr)
assert tee_repr_text.startswith('<itertools._tee object at 0x')
assert int(tee_repr_text.rsplit(' at ', 1)[1][:-1], 16) == id(tee_repr)
# `tee` itself is the module's one plain function, not a type.
assert str(type(itertools.tee)) == "<class 'builtin_function_or_method'>"

# === Errors ===
try:
    itertools.tee()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'tee expected at least 1 argument, got 0'

try:
    itertools.tee([1], 2, 3)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'tee expected at most 2 arguments, got 3'

# The blanket keyword rejection names the module as well as the function, which
# is CPython's wording here and nowhere else in `itertools`.
try:
    itertools.tee([1], n=2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'itertools.tee() takes no keyword arguments'

try:
    itertools.tee([1], -1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'n must be >= 0'

for bad_n in (1.0, 'x'):
    try:
        itertools.tee([1], bad_n)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f"'{type(bad_n).__name__}' object cannot be interpreted as an integer"

try:
    itertools.tee(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# An `n` whose tuple of iterators could never be allocated raises rather than
# being attempted.
try:
    itertools.tee([1], 2**62)
    assert False, 'expected MemoryError'
except MemoryError as exc:
    assert str(exc) == ''


# === Sources ===
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


assert [list(x) for x in itertools.tee(UpTo(3))] == [[1, 2, 3], [1, 2, 3]]


class Boom:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('boom')


erroring, _erroring_other = itertools.tee(Boom())
try:
    next(erroring)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'

# === Composition ===
evens, odds = itertools.tee(itertools.count())
assert list(itertools.islice(evens, 3)) == [0, 1, 2]
assert list(itertools.islice(odds, 3)) == [0, 1, 2]
assert [list(x) for x in itertools.tee(itertools.chain([1], [2]))] == [[1, 2], [1, 2]]
assert list(itertools.chain.from_iterable(itertools.tee([1, 2]))) == [1, 2, 1, 2]
assert [sum(x) for x in itertools.tee(range(4))] == [6, 6]
pairs_a, pairs_b = itertools.tee([1, 2, 3])
assert list(zip(pairs_a, pairs_b)) == [(1, 1), (2, 2), (3, 3)]
