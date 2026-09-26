# call-external
# === external functions are reachable from the implicit forms ===
assert eval('add_ints(1, 2)') == 3


def f():
    return eval('add_ints(2, 3)')


assert f() == 5
exec('r = add_ints(3, 4)')
assert r == 7

# === but not from an explicit namespace, matching CPython ===
try:
    exec('add_ints(1, 2)', {})
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'add_ints' is not defined"
