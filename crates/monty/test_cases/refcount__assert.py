# Passing comparison asserts retain their operands under a Dup2; this checks
# the success path releases them (production compile options, so the
# introspection bytecode is exercised).
x = [1, 2]
assert x == [1, 2]
assert x == [1, 2], 'with message'
assert x, 'truthy fallback path'
assert 'a' in 'abc'

# Failure path: a comparison that raises (list < int -> TypeError) leaves the
# retained operands on the stack; both the bare and message (Dup2) forms must
# release them on the exception path instead of leaking.
y = [3, 4]
try:
    assert y < 5
except TypeError:
    pass
try:
    assert y < 5, 'boom'
except TypeError:
    pass

x
# ref-counts={'x': 2, 'y': 1}
