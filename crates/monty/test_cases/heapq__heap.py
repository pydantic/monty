import heapq

# The list order of a heap is observable, so every operation must match CPython's algorithms.
heap = []
for x in [5, 3, 8, 1, 9, 2, 7, 3, 3, 0, 6]:
    heapq.heappush(heap, x)
assert heap == [0, 1, 2, 3, 3, 8, 7, 5, 3, 9, 6]

out = []
while heap:
    out.append(heapq.heappop(heap))
assert out == [0, 1, 2, 3, 3, 3, 5, 6, 7, 8, 9]

data = [9, 4, 7, 1, 8, 2, 2, 6, 0, 5, 3]
h = list(data)
assert heapq.heapify(h) is None
assert h == [0, 1, 2, 4, 3, 2, 7, 6, 9, 5, 8]

# heapreplace pops first then pushes; heappushpop pushes first then pops.
assert heapq.heapreplace(h, 100) == 0
assert h == [1, 3, 2, 4, 5, 2, 7, 6, 9, 100, 8]
assert heapq.heappushpop(h, 0) == 0
assert h == [1, 3, 2, 4, 5, 2, 7, 6, 9, 100, 8]
assert heapq.heappushpop(h, 50) == 1
assert h == [2, 3, 2, 4, 5, 50, 7, 6, 9, 100, 8]
assert heapq.heappushpop([], 4) == 4
assert heapq.heappushpop([3], 4) == 3

# Max-heap functions (3.14).
mh = list(data)
heapq.heapify_max(mh)
assert mh == [9, 8, 7, 6, 5, 2, 2, 1, 0, 4, 3]
heapq.heappush_max(mh, 10)
assert mh == [10, 8, 9, 6, 5, 7, 2, 1, 0, 4, 3, 2]
assert heapq.heappop_max(mh) == 10
assert mh == [9, 8, 7, 6, 5, 2, 2, 1, 0, 4, 3]
assert heapq.heapreplace_max(mh, -1) == 9
assert mh == [8, 6, 7, 1, 5, 2, 2, -1, 0, 4, 3]
assert heapq.heappushpop_max(mh, 100) == 100
assert heapq.heappushpop_max(mh, 0) == 8
assert mh == [7, 6, 2, 1, 5, 2, 0, -1, 0, 4, 3]

# Tuples, strings, floats and mixed int/float compare through normal rich comparison.
tasks = []
for item in [(2, 'b'), (1, 'z'), (1, 'a'), (3, 'c')]:
    heapq.heappush(tasks, item)
assert [heapq.heappop(tasks) for _ in range(4)] == [(1, 'a'), (1, 'z'), (2, 'b'), (3, 'c')]

words = ['pear', 'apple', 'fig', 'banana']
heapq.heapify(words)
assert words == ['apple', 'banana', 'fig', 'pear']

mixed = [2.5, 1, 3, 0.5]
heapq.heapify(mixed)
assert mixed == [0.5, 1, 3, 2.5]


# Edge sizes.
for size in range(0, 8):
    for seq in [list(range(size)), list(range(size, 0, -1)), [1] * size]:
        h = list(seq)
        heapq.heapify(h)
        assert [heapq.heappop(h) for _ in range(len(h))] == sorted(seq)

# The heap is mutated in place.
single = [7]
assert heapq.heappop(single) == 7
assert single == []
