# Tests reference counting when building a set gives up partway through.
#
# A set literal, `set(iterable)` and `set.update` each take ownership of their
# items — or, for a set source, of copies of its entries — before the insertion
# that raises. Whatever the operation never reached has to be released on the
# way out. The exception is identical either way, so only the counts here pin it.

first = ('first',)
last = ('last',)
unhashable = []

# `first` is already inside the half-built set when the list is rejected, and
# `last` has not been reached yet: both paths have to be released.
try:
    {first, unhashable, last}
    assert False, 'the set literal should reject the list element'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a set element (unhashable type: 'list')"

try:
    set([first, unhashable, last])
    assert False, 'set(iterable) should reject the list element'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a set element (unhashable type: 'list')"

try:
    frozenset([first, unhashable, last])
    assert False, 'frozenset(iterable) should reject the list element'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a set element (unhashable type: 'list')"


class Tripwire:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        raise ValueError('tripped')


class Colliding:
    def __hash__(self):
        return 0


# `update` copies the source's entries out before inserting them, so the very
# first insertion raising leaves both copies for the operation to release.
source_first = Colliding()
source_last = Colliding()
source = {source_first, source_last}
target = {Tripwire()}

try:
    target.update(source)
    assert False, 'the update should trip on the first colliding entry'
except ValueError as e:
    assert str(e) == 'tripped'

assert len(target) == 1
assert len(source) == 2
# ref-counts={'first': 1, 'last': 1, 'unhashable': 1, 'Tripwire': 2, 'Colliding': 3, 'source_first': 2, 'source_last': 2, 'source': 1, 'target': 1}
