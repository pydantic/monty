import collections
from collections import Counter

# === Imports and empty constructor ===
empty = collections.Counter()
assert empty == {}, 'Counter() returns an empty dict'

# === Iterable counting ===
chars = collections.Counter('abca')
assert chars == {'a': 2, 'b': 1, 'c': 1}, 'iterable source counts each element'

# === Mapping source counts ===
mapping = collections.Counter({'a': 2, 'b': 3})
assert mapping == {'a': 2, 'b': 3}, 'mapping source uses values as counts'

# === Keyword counts ===
kwargs_only = collections.Counter(a=2, b=1)
assert kwargs_only == {'a': 2, 'b': 1}, 'kwargs are interpreted as key=count'

# === Mixed positional and kwargs ===
mixed = collections.Counter('a', b=2)
assert mixed == {'a': 1, 'b': 2}, 'kwargs counts are added after iterable counting'

merged = collections.Counter({'a': 2}, a=1)
assert merged == {'a': 3}, 'kwargs merge by addition with mapping counts'

# === from-import alias ===
alias = Counter('zzz')
assert alias == {'z': 3}, 'from collections import Counter binds callable constructor'

# === Errors: non-iterable source ===
try:
    collections.Counter(1)
    assert False, 'Counter(1) should raise TypeError for non-iterable source'
except TypeError as e:
    assert str(e) == "'int' object is not iterable", 'Counter(1) error message matches'

# === Errors: too many positional args ===
try:
    collections.Counter([], [])
    assert False, 'Counter with two positional args should raise TypeError'
except TypeError as e:
    assert str(e) == 'Counter.__init__() takes from 1 to 2 positional arguments but 3 were given', (
        'positional arg count error matches'
    )

# === Errors: unhashable keys ===
try:
    collections.Counter([[1], [1]])
    assert False, 'Counter with unhashable keys should raise TypeError'
except TypeError as e:
    assert str(e) == "unhashable type: 'list'", 'unhashable key error matches'
