# call-external
# Callable-valued dataclass fields must be invoked via attr-call syntax
# (issue #352): `box.foo(...)` should behave like `tmp = box.foo; tmp(...)`.

box = make_callable_box()

# === callable field is invoked via attr-call syntax ===
assert box.foo(2, 3) == 5, 'callable dataclass field invoked directly via attr-call'

# === same result when loaded into a variable first ===
f = box.foo
assert f(10, 20) == 30, 'callable field works when loaded into a variable first'

# === non-callable field still raises TypeError ===
try:
    box.data()
    assert False, 'expected box.data() to raise TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not callable", f'wrong message: {e}'
