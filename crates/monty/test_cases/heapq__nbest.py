import heapq

# nlargest / nsmallest: results, ordering and tie-breaking.
data = [5, 1, 8, 3, 9, 2, 8, 7, 3, 0]
assert heapq.nlargest(3, data) == [9, 8, 8]
assert heapq.nsmallest(3, data) == [0, 1, 2]
assert heapq.nlargest(1, data) == [9]
assert heapq.nsmallest(1, data) == [0]
assert heapq.nlargest(0, data) == []
assert heapq.nsmallest(-2, data) == []
assert heapq.nlargest(100, data) == sorted(data, reverse=True)
assert heapq.nsmallest(100, data) == sorted(data)
assert heapq.nlargest(3, []) == []
assert heapq.nsmallest(1, []) == []

# Ties keep the earliest item first, with and without a key (equal keys are never compared by value).
pairs = [('a', 2), ('b', 3), ('c', 3), ('d', 1), ('e', 3), ('f', 2), ('g', 1)]
assert heapq.nlargest(2, pairs, key=lambda p: p[1]) == [('b', 3), ('c', 3)]
assert heapq.nlargest(4, pairs, key=lambda p: p[1]) == [('b', 3), ('c', 3), ('e', 3), ('a', 2)]
assert heapq.nsmallest(3, pairs, key=lambda p: p[1]) == [('d', 1), ('g', 1), ('a', 2)]
assert heapq.nsmallest(1, pairs, key=lambda p: p[1]) == [('d', 1)]
assert heapq.nlargest(1, pairs, key=lambda p: p[1]) == [('b', 3)]
assert heapq.nlargest(7, pairs, key=lambda p: p[1]) == sorted(pairs, key=lambda p: p[1], reverse=True)


class NoCompare:
    def __init__(self, name):
        self.name = name

    def __lt__(self, other):
        raise AssertionError('items must not be compared when a key is given')


items = [NoCompare(i) for i in range(6)]
best = heapq.nlargest(3, items, key=lambda o: o.name % 2)
assert [o.name for o in best] == [1, 3, 5]
best = heapq.nsmallest(2, items, key=lambda o: o.name % 2)
assert [o.name for o in best] == [0, 2]

# Any iterable works, including generators that cannot report their length.
assert heapq.nlargest(2, (x for x in data)) == [9, 8]
assert heapq.nsmallest(2, iter(data)) == [0, 1]
assert heapq.nsmallest(3, 'hello') == ['e', 'h', 'l']
assert heapq.nlargest(2, {3: 'a', 1: 'b', 2: 'c'}) == [3, 2]
assert heapq.nlargest(2, range(10)) == [9, 8]
assert heapq.nsmallest(2, [(2, 'b'), (1, 'z'), (1, 'a')]) == [(1, 'a'), (1, 'z')]
assert heapq.nlargest(2, ['b', 'a', 'c'], key=None) == ['c', 'b']

# Cross-check against sorted() on many shapes, with duplicate-heavy keys.
for size in range(0, 14):
    seq = [(i * 7 + 3) % 5 for i in range(size)]
    tagged = list(enumerate(seq))
    for n in range(0, size + 2):
        assert heapq.nsmallest(n, tagged, key=lambda p: p[1]) == sorted(tagged, key=lambda p: p[1])[:n]
        assert heapq.nlargest(n, tagged, key=lambda p: p[1]) == sorted(tagged, key=lambda p: p[1], reverse=True)[:n]
        assert heapq.nsmallest(n, seq) == sorted(seq)[:n]
        assert heapq.nlargest(n, seq) == sorted(seq, reverse=True)[:n]

# The key is called exactly once per item (and on every item when n == 1).
calls = []


def key(x):
    calls.append(x)
    return -x


assert heapq.nsmallest(1, [4, 2, 6], key=key) == [6]
assert calls == [4, 2, 6]
calls.clear()
assert heapq.nlargest(2, iter([4, 2, 6, 1]), key=key) == [1, 2]
assert calls == [4, 2, 6, 1]

# `n == 1` takes the `min` / `max` path before `n` is used as an integer, so `1.0` works, as in CPython.
assert heapq.nsmallest(1.0, [3, 1, 2]) == [1]
assert heapq.nlargest(1.0, [3, 1, 2]) == [3]
