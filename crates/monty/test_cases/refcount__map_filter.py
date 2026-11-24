import gc


def make_callback():
    state = [1]
    return lambda item: item if state else None


# Both the callback and the sources are reachable only through these adaptors.
mapped = map(make_callback(), [[1], [2]])
filtered = filter(make_callback(), [[3], [4]])
next(mapped)
next(filtered)
gc.collect()
assert next(mapped) == [2]
assert next(filtered) == [4]

# Releasing an adaptor must release its unnamed callback and sources.
dropped = map(make_callback(), [[5]])
dropped = None
dropped = filter(make_callback(), [[6]])
dropped = None

# Live cycles exercise source tracing; abandoned cycles exercise callback tracing.
cyclic = []
cyclic.append(map(make_callback(), cyclic))
cyclic.append(filter(make_callback(), cyclic))


def abandon_cycles():
    state = []
    state.append(map(lambda item: state, [1]))
    state.append(filter(lambda item: state, [1]))


abandon_cycles()
gc.collect()

# A later source failing acquisition must release already-acquired sources.
try:
    map(make_callback(), [[7]], None)
    assert False, 'expected non-iterable source to fail'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not iterable"

# ref-counts={'gc': 1, 'mapped': 1, 'filtered': 1, 'cyclic': 3}
