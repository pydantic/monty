# Tests reference counting when building a dict gives up partway through.
#
# A dict literal, `dict(**mapping)` and `{**mapping}` each take ownership of
# their keys and values — or, for a mapping source, of copies of its entries —
# before the insertion that raises. Whatever the operation never reached has to
# be released on the way out. The exception is identical either way, so only the
# counts here pin it.

first_key = ('first',)
first_value = ('first-value',)
last_key = ('last',)
last_value = ('last-value',)
unhashable = []

# `first_key` / `first_value` are already inside the half-built dict when the list
# is rejected, and `last_key` / `last_value` have not been reached yet: both paths
# have to be released.
try:
    {first_key: first_value, unhashable: 1, last_key: last_value}
    assert False, 'the dict literal should reject the list key'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a dict key (unhashable type: 'list')"

try:
    dict([(first_key, first_value), (unhashable, 1), (last_key, last_value)])
    assert False, 'dict(pairs) should reject the list key'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a dict key (unhashable type: 'list')"

try:
    dict(**{first_key: first_value, unhashable: 1, last_key: last_value})
    assert False, 'dict(**mapping) should reject the list key'
except TypeError as e:
    assert str(e) == "cannot use 'list' as a dict key (unhashable type: 'list')"


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
source = {source_first: 1, source_last: 2}
target = {Tripwire(): 3}

try:
    target.update(source)
    assert False, 'the update should trip on the first colliding entry'
except ValueError as e:
    assert str(e) == 'tripped'

assert len(target) == 1
assert len(source) == 2
# ref-counts={'first_key': 1, 'first_value': 1, 'last_key': 1, 'last_value': 1, 'unhashable': 1, 'Tripwire': 2, 'Colliding': 3, 'source_first': 2, 'source_last': 2, 'source': 1, 'target': 1}
