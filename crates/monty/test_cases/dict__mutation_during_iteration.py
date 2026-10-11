# Deleting an entry leaves a tombstone rather than shifting later entries down,
# so a walk in progress keeps its place. CPython's two guards apply: the size
# check, and the one that fires when a live entry turns up after the walk has
# been handed everything it was owed.

# === a delete that keeps the size raises, as it renumbers under the cursor ===
d = {'a': 1, 'b': 2, 'c': 3, 'd': 4}
seen = []
try:
    for k in d:
        seen.append(k)
        if k == 'b':
            d.pop('a')
            d['z'] = 9
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary keys changed during iteration'
assert seen == ['a', 'b', 'c', 'd']

# === a delete ahead of the cursor completes, and nothing is skipped ===
d = {'a': 1, 'b': 2, 'c': 3}
seen = []
for k in d:
    seen.append(k)
    if k == 'a':
        d.pop('c')
        d['z'] = 9
assert seen == ['a', 'b', 'z']

# === shrinking alone is the size error ===
d = {'a': 1, 'b': 2, 'c': 3}
seen = []
try:
    for k in d:
        seen.append(k)
        d.pop('a')
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'
assert seen == ['a']

# === the same guards apply to .items() and a comprehension ===
d = {'a': 1, 'b': 2, 'c': 3, 'd': 4}
seen = []
try:
    for k, v in d.items():
        seen.append(k)
        if k == 'b':
            d.pop('a')
            d['z'] = 9
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary keys changed during iteration'
assert seen == ['a', 'b', 'c', 'd']

# === a tombstoned dict still reports the right length and contents ===
d = {'a': 1, 'b': 2, 'c': 3}
d.pop('a')
d['z'] = 9
assert len(d) == 3
assert list(d) == ['b', 'c', 'z']
assert repr(d) == "{'b': 2, 'c': 3, 'z': 9}"
assert d.popitem() == ('z', 9)
assert list(d.items()) == [('b', 2), ('c', 3)]

# === deleting every entry and refilling reuses the storage cleanly ===
d = {}
for i in range(20):
    d[i] = i
for i in range(20):
    d.pop(i)
assert len(d) == 0
assert list(d) == []
for i in range(5):
    d[i] = i * 2
assert list(d.items()) == [(0, 0), (1, 2), (2, 4), (3, 6), (4, 8)]

# === a dict shrinking under copy() keeps every surviving entry ===
d = {'a': 1, 'b': 2, 'c': 3}
d.pop('b')
assert d.copy() == {'a': 1, 'c': 3}
assert dict(d) == {'a': 1, 'c': 3}

# === sets keep their remaining members across a removal mid-iteration ===
# Ints hash to themselves, so iteration order is stable on both engines.
s = {0, 1, 2, 3}
seen = []
try:
    for k in s:
        seen.append(k)
        if len(seen) == 2:
            s.discard(0)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'Set changed size during iteration'
assert seen == [0, 1]

# === an exhausted set iterator stays exhausted when the set grows ===
s = {1}
it = iter(s)
assert list(it) == [1]
s.add(2)
assert list(it) == []

# === a size error is sticky, even once the size is restored ===
d = {'a': 1, 'b': 2, 'c': 3}
it = iter(d)
next(it)
d['x'] = 0
for _ in range(2):
    try:
        next(it)
        assert False, 'expected RuntimeError'
    except RuntimeError as exc:
        assert str(exc) == 'dictionary changed size during iteration'
    d.pop('x', None)

s = {1, 2, 3}
it = iter(s)
next(it)
s.add(99)
for _ in range(2):
    try:
        next(it)
        assert False, 'expected RuntimeError'
    except RuntimeError as exc:
        assert str(exc) == 'Set changed size during iteration'
    s.discard(99)

# === "keys changed" ends the iterator ===
d = {'a': 1, 'b': 2}
it = iter(d)
next(it)
d.pop('a')
d['z'] = 0
assert next(it) == 'b'
try:
    next(it)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary keys changed during iteration'
assert list(it) == []

# === a tombstoned set still reports the right length and contents ===
s = {0, 1, 2, 3}
s.discard(0)
s.add(99)
assert len(s) == 4
assert sorted(s) == [1, 2, 3, 99]
assert sorted(s.copy()) == [1, 2, 3, 99]
assert 1 in s
assert 0 not in s

# === compaction drops the tombstones once they reach half the slots ===
# Only `push` compacts, so the insert below is what renumbers the survivors.
d = {}
for i in range(12):
    d[i] = i
for i in range(8):
    d.pop(i)
d[100] = 100
assert len(d) == 5
assert list(d) == [8, 9, 10, 11, 100]

# === a compaction under a live cursor does not skip an entry ===
# The deletes shrink the dict, so the size guard fires before the renumbering
# can be observed — the walk never silently drops the tail.
d = {}
for i in range(12):
    d[i] = i
seen = []
try:
    for k in d:
        seen.append(k)
        if k == 0:
            for j in range(1, 9):
                d.pop(j)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'dictionary changed size during iteration'
assert seen == [0]

# === a same-size delete/insert round trip leaves the dict intact ===
d = {chr(97 + i): i for i in range(10)}
for r in range(50):
    k = chr(97 + (r % 10))
    d.pop(k)
    d[k] = r
assert len(d) == 10
assert sorted(d) == [chr(97 + i) for i in range(10)]

# === the same compaction shape on a set ===
s = set(range(12))
for i in range(8):
    s.discard(i)
s.add(100)
assert sorted(s) == [8, 9, 10, 11, 100]
