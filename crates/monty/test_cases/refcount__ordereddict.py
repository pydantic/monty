# Reference counting for `collections.OrderedDict`.
#
# An OrderedDict holds one reference to each key and value, like a dict. The
# operations that reorder or rebuild the table (move_to_end, popitem, copy, `|`)
# must neither leak nor double-release them, including when the call fails.
# `shared` is bound by its own variable plus the one reference per container
# that still holds it at the end.

from collections import OrderedDict

shared = [1, 2, 3]
od = OrderedDict([('a', shared), ('b', shared), ('c', shared)])
od.move_to_end('a')
od.move_to_end('c', last=False)

# popitem returns the entry; the popped tuple is dropped, releasing its clone.
od.popitem()
od.popitem(last=False)

# A missing key fails after the key and `last` are bound.
try:
    od.move_to_end('missing', False)
    assert False, 'expected KeyError'
except KeyError:
    pass

# copy / fromkeys / `|` build new tables holding fresh clones.
cp = od.copy()
merged = od | {'z': shared}
od_other = OrderedDict.fromkeys(['x', 'y'], shared)
cp = None
merged = None
od_other = None

shared
# ref-counts={'shared': 3, 'od': 1}
