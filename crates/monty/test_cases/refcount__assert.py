# Passing comparison asserts retain their operands under a Dup2; this checks
# the success path releases them (production compile options, so the
# introspection bytecode is exercised).
x = [1, 2]
assert x == [1, 2]
assert x == [1, 2], 'with message'
assert x, 'truthy fallback path'
assert 'a' in 'abc'
x
# ref-counts={'x': 2}
