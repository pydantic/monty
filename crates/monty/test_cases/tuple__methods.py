# === tuple.index() ===
t = (1, 2, 3, 2)
assert t.index(2) == 1, 'index finds first occurrence'
assert t.index(3) == 2, 'index finds element'
assert t.index(2, 2) == 3, 'index with start'
assert t.index(2, 1, 4) == 1, 'index with start and end'

t = ('a', 'b', 'c')
assert t.index('b') == 1, 'index string in tuple'

# === tuple.count() ===
t = (1, 2, 2, 3, 2)
assert t.count(2) == 3, 'count multiple occurrences'
assert t.count(1) == 1, 'count single occurrence'
assert t.count(4) == 0, 'count zero occurrences'
assert ().count(1) == 0, 'count on empty tuple'

t = ('a', 'b', 'a')
assert t.count('a') == 2, 'count strings'
