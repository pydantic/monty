# iter(callable, sentinel) must not leak its callable or sentinel, and must
# release both the moment the sentinel is seen — CPython's calliter_iternext
# `Py_CLEAR`s them, so a spent-but-still-referenced iterator does not pin them.
#
# `stop` is the heap-allocated sentinel, so its count is observable: it ends at
# 2, the global binding plus its slot in `data`. Were the exhausted iterator
# still holding it, `stop` would be 3. `src` ends at 1 (its global binding
# alone) and `data` at 2 (the global plus `src`'s slot). The lambda `f` is not
# heap allocated, so it does not appear in the map.
stop = [9]
data = [7, 8, stop]
src = iter(data)
f = lambda: next(src)
it = iter(f, stop)
out = list(it)
out
# ref-counts={'stop': 2, 'data': 2, 'src': 1, 'it': 1, 'out': 2}
