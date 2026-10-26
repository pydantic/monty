# A tight loop of cheap builtin-method calls whose cost is dominated by
# `#[derive(FromArgs)]` binding (positional fill, kwarg matching, defaults).

r = 0
for i in range(10_000):
    s = 'ab'.replace('a', 'b')
    t = 'x'.encode('utf-8', 'strict')
    u = sorted([3, 1, 2], reverse=False)
    r += len(s) + len(t) + len(u)
assert r == 60_000
