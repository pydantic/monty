# `itertools.groupby`: runs of equal keys, each yielded as `(key, group)` where
# the group is a sub-iterator sharing the parent's position in the source.
import itertools

# === Basics ===
assert [(k, list(g)) for k, g in itertools.groupby('AAAABBBCCDAABBB')] == [
    ('A', ['A', 'A', 'A', 'A']),
    ('B', ['B', 'B', 'B']),
    ('C', ['C', 'C']),
    ('D', ['D']),
    ('A', ['A', 'A']),
    ('B', ['B', 'B', 'B']),
]
assert [k for k, g in itertools.groupby('AAAABBBCCD')] == ['A', 'B', 'C', 'D']
assert [(k, list(g)) for k, g in itertools.groupby([1, 1, 2, 3, 3, 3])] == [(1, [1, 1]), (2, [2]), (3, [3, 3, 3])]
assert list(itertools.groupby([])) == []
assert [(k, list(g)) for k, g in itertools.groupby([5])] == [(5, [5])]
# Only consecutive equal keys group, so unsorted input repeats keys.
assert [k for k, g in itertools.groupby([1, 2, 1, 2])] == [1, 2, 1, 2]
# Equality, not identity: `1 == 1.0 == True` is one group whose key is the
# first item seen.
assert [(k, list(g)) for k, g in itertools.groupby([1, 1.0, True, 2])] == [(1, [1, 1.0, True]), (2, [2])]

# === Key functions ===
assert [(k, list(g)) for k, g in itertools.groupby([1, 2, 3, 4, 5, 6], key=lambda x: x // 3)] == [
    (0, [1, 2]),
    (1, [3, 4, 5]),
    (2, [6]),
]
assert [(k, list(g)) for k, g in itertools.groupby('aAbBB', lambda c: c.upper())] == [
    ('A', ['a', 'A']),
    ('B', ['b', 'B', 'B']),
]
assert [(k, list(g)) for k, g in itertools.groupby(['ab', 'cd', 'e'], len)] == [(2, ['ab', 'cd']), (1, ['e'])]
# A `None` key groups by the items themselves, however it is given.
assert [k for k, g in itertools.groupby([1, 1, 2], None)] == [1, 2]
assert [k for k, g in itertools.groupby([1, 1, 2], key=None)] == [1, 2]
assert [k for k, g in itertools.groupby(iterable=[1, 1, 2])] == [1, 2]
# The key is what is yielded, not the item.
assert [k for k, g in itertools.groupby([-1, 1, -2], abs)] == [1, 2]

# The key function is called once per item, in order, as each is read.
seen = []


def tracked(x):
    seen.append(x)
    return x % 2


grouped = itertools.groupby([1, 3, 2], tracked)
assert seen == []
key, group = next(grouped)
assert (key, seen) == (1, [1])
assert list(group) == [1, 3]
# Reading past the group's end reads the next item in, which is what ends it.
assert seen == [1, 3, 2]
assert next(grouped)[0] == 0
assert seen == [1, 3, 2]

# === Sharing the source ===
# Advancing the parent spends the group it yielded: it becomes empty rather
# than resuming from wherever the parent has got to.
grouped = itertools.groupby('AABBB')
key_a, group_a = next(grouped)
key_b, group_b = next(grouped)
assert (key_a, key_b) == ('A', 'B')
assert list(group_a) == []
assert list(group_b) == ['B', 'B', 'B']

# The same, part-way through a group: the item already read in goes to the
# parent's skip, not to the stale group.
grouped = itertools.groupby([1, 1, 2, 2])
key_one, group_one = next(grouped)
assert next(group_one) == 1
key_two, group_two = next(grouped)
assert key_two == 2
assert list(group_one) == []
assert list(group_two) == [2, 2]

# Releasing each group before the next opens lets its heap slot be reused by
# the next one, so the "is this still the current group?" check must not
# confuse a fresh group with the spent one it replaced.
grouped = itertools.groupby([1, 2, 3])
reused_group = None
for expected in (1, 2, 3):
    # Released BEFORE the next pair is opened: rebinding alone would keep the
    # old group alive across the `next` that allocates its replacement, so the
    # slot could not be reused and the check below would prove nothing.
    reused_group = None
    reused_key, reused_group = next(grouped)
    assert reused_key == expected
    assert list(reused_group) == [expected]

# Collecting every pair first and reading the groups afterwards leaves them all
# stale, so each is empty however long the run was.
kept = [(k, g) for k, g in itertools.groupby('aabb')]
assert [k for k, _ in kept] == ['a', 'b']
assert [list(g) for _, g in kept] == [[], []]

# A group kept alive outlives the parent binding, holding the parent through it.
first_group = next(itertools.groupby([7, 7, 8]))[1]
assert list(first_group) == [7, 7]

# The parent's skip runs to the next key, so a group never drained is skipped
# in full.
grouped = itertools.groupby([1, 1, 1, 2, 3])
next(grouped)
assert next(grouped)[0] == 2
assert next(grouped)[0] == 3

# An exhausted group and parent stay exhausted.
grouped = itertools.groupby([1, 2])
key_one, group_one = next(grouped)
assert list(group_one) == [1]
assert list(group_one) == []
key_two, group_two = next(grouped)
assert list(group_two) == [2]
assert list(grouped) == []
for spent in (grouped, group_one, group_two):
    try:
        next(spent)
        assert False, 'expected StopIteration'
    except StopIteration:
        pass

# The parent reads one item ahead, so a partially consumed source is left
# just past the item that ended the last group.
source = iter([1, 1, 2, 3])
grouped = itertools.groupby(source)
key, group = next(grouped)
assert list(group) == [1, 1]
assert list(source) == [3]


# A key function that advances the parent from inside a group's own step
# leaves that group still yielding: the "is this still the current group?" test
# happens when the group is entered, not again after the key call.
def reentering_groupby(source, trigger):
    holder = [None]
    calls = [0]
    depth = [0]
    seen = []

    def key(item):
        calls[0] += 1
        if calls[0] == trigger and depth[0] == 0 and holder[0] is not None:
            depth[0] += 1
            seen.append(next(holder[0], ('stop', None))[0])
            depth[0] -= 1
        return item

    grouped = itertools.groupby(source, key)
    holder[0] = grouped
    first_key, first_group = next(grouped)
    return first_key, [next(first_group, 'STOP') for _ in range(3)], seen


assert reentering_groupby([1, 1, 1, 1], 2) == (1, [1, 1, 'STOP'], ['stop'])
assert reentering_groupby([1, 1, 2, 1], 2) == (1, [1, 1, 'STOP'], [2])
assert reentering_groupby([1, 1, 2, 2, 1], 3) == (1, [1, 1, 'STOP'], [2])

# === Iterator protocol and types ===
grouped = itertools.groupby([1])
assert iter(grouped) is grouped
key, group = next(grouped)
assert iter(group) is group
assert type(next(itertools.groupby([1]))) is tuple
assert str(type(grouped)) == "<class 'itertools.groupby'>"
assert type(grouped).__name__ == 'groupby'
assert str(type(group)) == "<class 'itertools._grouper'>"
assert type(group).__name__ == '_grouper'
groupby_repr_text = repr(grouped)
assert groupby_repr_text.startswith('<itertools.groupby object at 0x')
assert int(groupby_repr_text.rsplit(' at ', 1)[1][:-1], 16) == id(grouped)
group_repr_text = repr(group)
assert group_repr_text.startswith('<itertools._grouper object at 0x')
assert int(group_repr_text.rsplit(' at ', 1)[1][:-1], 16) == id(group)

# === Signature errors ===
try:
    itertools.groupby()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "groupby() missing required argument 'iterable' (pos 1)"

try:
    itertools.groupby([1], None, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'groupby() takes at most 2 arguments (3 given)'

# Arity counts positionals and keywords together...
try:
    itertools.groupby([1, 2], key=None, x=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'groupby() takes at most 2 arguments (3 given)'

# ...and only within it is a keyword checked by name.
try:
    itertools.groupby([1], keyfunc=None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "groupby() got an unexpected keyword argument 'keyfunc'"

# The iterable is resolved eagerly, so a non-iterable raises up front...
try:
    itertools.groupby(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# ...while a non-callable key is discovered when the first item is keyed.
unkeyable = itertools.groupby([1], 5)
try:
    next(unkeyable)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"


# === Exceptions propagate ===
def explode(x):
    raise ValueError('bang')


# The item is heap-allocated, so a `groupby` that dropped it on this path would
# show up as a leaked reference under `memory-model-checks`.
try:
    next(itertools.groupby([[1]], explode))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'bang'


class Boom:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('boom')


try:
    next(itertools.groupby(Boom()))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'

# Raised from the group too, since it is the group that reads the source.
grouped = itertools.groupby(itertools.chain([1], Boom()))
key, group = next(grouped)
assert next(group) == 1
try:
    next(group)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'


# The group's key is the LEFT operand of the comparison, as CPython's
# `PyObject_RichCompareBool(tgtkey, currkey, Py_EQ)` makes it — visible only
# with an asymmetric `__eq__`.
class AlwaysEqual:
    def __eq__(self, other):
        return True


class NeverEqual:
    def __eq__(self, other):
        return False


# The first item's key is the target, so its `__eq__` decides every run.
assert [len(list(g)) for k, g in itertools.groupby([AlwaysEqual(), NeverEqual(), AlwaysEqual()], lambda x: x)] == [3]
assert [len(list(g)) for k, g in itertools.groupby([NeverEqual(), AlwaysEqual()], lambda x: x)] == [1, 1]


# A key comparison that raises propagates from whichever side compares.
class Unequal:
    def __eq__(self, other):
        raise ValueError('no comparing')


grouped = itertools.groupby([Unequal(), Unequal()])
key, group = next(grouped)
assert next(group) is key
try:
    next(group)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'no comparing'


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


assert [(k, list(g)) for k, g in itertools.groupby(UpTo(4), lambda x: x > 2)] == [(False, [1, 2]), (True, [3, 4])]

# === Composition ===
assert [k for k, g in itertools.groupby(itertools.islice(itertools.cycle('ab'), 5))] == ['a', 'b', 'a', 'b', 'a']
assert [len(list(g)) for k, g in itertools.groupby(sorted('mississippi'))] == [4, 1, 2, 4]
assert {k: len(list(g)) for k, g in itertools.groupby(sorted('mississippi'))} == {'i': 4, 'm': 1, 'p': 2, 's': 4}
assert max(itertools.groupby([1, 1, 2]), key=lambda kg: kg[0])[0] == 2
# Bounding an infinite source: `islice` over the groups stops the parent's
# skip from running on.
assert [k for k, g in itertools.islice(itertools.groupby(itertools.count(), lambda x: x // 2), 3)] == [0, 1, 2]

# Unpacking in a for loop, draining each group as it goes.
lengths = []
for key, group in itertools.groupby('aabccc'):
    lengths.append((key, len(list(group))))
assert lengths == [('a', 2), ('b', 1), ('c', 3)]
