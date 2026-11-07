import heapq


def raises(exc, message, func, *args, **kwargs):
    try:
        func(*args, **kwargs)
    except exc as e:
        assert str(e) == message, (str(e), message)
    else:
        assert False, 'did not raise'


# Non-list heaps and empty heaps.
raises(TypeError, 'heappush() argument 1 must be list, not tuple', heapq.heappush, (1,), 2)
raises(TypeError, 'heappush() argument 1 must be list, not None', heapq.heappush, None, 2)
raises(TypeError, 'heappop() argument must be list, not tuple', heapq.heappop, (1,))
raises(TypeError, 'heapify() argument must be list, not str', heapq.heapify, 'abc')
raises(TypeError, 'heapreplace() argument 1 must be list, not int', heapq.heapreplace, 1, 2)
raises(TypeError, 'heappushpop() argument 1 must be list, not dict', heapq.heappushpop, {}, 2)
raises(TypeError, 'heappush_max() argument 1 must be list, not tuple', heapq.heappush_max, (1,), 2)
raises(TypeError, 'heappop_max() argument must be list, not tuple', heapq.heappop_max, (1,))
raises(IndexError, 'index out of range', heapq.heappop, [])
raises(IndexError, 'index out of range', heapq.heappop_max, [])
raises(IndexError, 'index out of range', heapq.heapreplace, [], 1)
raises(IndexError, 'index out of range', heapq.heapreplace_max, [], 1)

# Argument counts and keywords.
raises(TypeError, 'heappush expected 2 arguments, got 1', heapq.heappush, [])
raises(TypeError, 'heappush expected 2 arguments, got 3', heapq.heappush, [], 1, 2)
raises(TypeError, '_heapq.heappop() takes exactly one argument (0 given)', heapq.heappop)
raises(TypeError, '_heapq.heappop() takes exactly one argument (2 given)', heapq.heappop, [], [])
raises(TypeError, '_heapq.heappush() takes no keyword arguments', heapq.heappush, [], item=1)
raises(TypeError, '_heapq.heappop() takes no keyword arguments', heapq.heappop, heap=[])

# nlargest / nsmallest argument errors.
raises(TypeError, "'int' object is not iterable", heapq.nlargest, 1, 5)
raises(TypeError, "'int' object is not iterable", heapq.nsmallest, 2, 5)
raises(TypeError, "'int' object is not callable", heapq.nlargest, 2, [1, 2, 3], 2)
raises(TypeError, "'int' object is not callable", heapq.nsmallest, 2, [1, 2, 3], key=2)

# Errors from comparisons and keys propagate.
raises(TypeError, "'<' not supported between instances of 'str' and 'int'", heapq.heappush, [1], 'a')
raises(TypeError, "'<' not supported between instances of 'str' and 'int'", heapq.heapify, [1, 'a', 2])
raises(TypeError, "'<' not supported between instances of 'str' and 'int'", heapq.heappush_max, ['a'], 1)


raises(TypeError, "'<' not supported between instances of 'int' and 'str'", heapq.nlargest, 2, [1, 'a', 2])
raises(TypeError, "'<' not supported between instances of 'str' and 'int'", heapq.nsmallest, 2, [1, 'a', 2])
raises(TypeError, "'<' not supported between instances of 'int' and 'str'", heapq.heappushpop, [1], 'a')


def bad_key(x):
    if x == 3:
        raise KeyError('bad')
    return x


try:
    heapq.nlargest(2, [1, 2, 3, 4], key=bad_key)
except KeyError as e:
    assert e.args == ('bad',)
else:
    assert False

try:
    heapq.nsmallest(1, [1, 2, 3, 4], key=bad_key)
except KeyError as e:
    assert e.args == ('bad',)
else:
    assert False

# Large heaps use a different traversal order in heapify; results must still match a sorted list.
big = [(i * 7919) % 10007 for i in range(5000)]
h = list(big)
heapq.heapify(h)
assert h[0] == min(big)
assert all(h[(i - 1) // 2] <= h[i] for i in range(1, len(h)))
assert [heapq.heappop(h) for _ in range(5000)] == sorted(big)

hm = list(big)
heapq.heapify_max(hm)
assert hm[0] == max(big)
assert all(hm[(i - 1) // 2] >= hm[i] for i in range(1, len(hm)))
assert [heapq.heappop_max(hm) for _ in range(5000)] == sorted(big, reverse=True)

assert heapq.nlargest(5, big) == sorted(big, reverse=True)[:5]
assert heapq.nsmallest(5, big) == sorted(big)[:5]
assert list(heapq.merge(sorted(big[:2500]), sorted(big[2500:]))) == sorted(big)

# Exact layout of a heap just above the cache-friendly threshold matches CPython.
layout = [(i * 31) % 2601 for i in range(2601)]
heapq.heapify(layout)
assert layout[:12] == [0, 3, 1, 8, 6, 15, 2, 13, 11, 12, 20, 26], layout[:12]
assert layout[-6:] == [2415, 2446, 2480, 2508, 2539, 2570], layout[-6:]
assert sum(i * v for i, v in enumerate(layout)) == 5187294727
