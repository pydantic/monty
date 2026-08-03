# Unbound closure-cell reads: CPython raises the free-variable NameError only
# in a frame that *captured* the cell; the frame that owns the cell raises
# UnboundLocalError, like any other unassigned local.


# === reading a nonlocal before assignment raises NameError in the inner frame ===
def outer():
    def inner():
        nonlocal x
        return x  # x not yet defined

    result = inner()
    x = 10
    return result


try:
    outer()
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "cannot access free variable 'x' where it is not associated with a value in enclosing scope"


# === the owning frame reads its own unbound cell: UnboundLocalError ===
def owner_reads_unbound_cell():
    def inner():
        return y

    y
    y = 1


try:
    owner_reads_unbound_cell()
    assert False, 'expected UnboundLocalError'
except UnboundLocalError as e:
    assert str(e) == "cannot access local variable 'y' where it is not associated with a value"
