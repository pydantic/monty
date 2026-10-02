# call-external
# An import of a module the sandbox does not have asks the host, which answers
# with an object whose attributes are the tools; `tools` is the harness fixture.
import tools
import tools as t
from tools import add_ints, concat_strings as concat

# === calling through the module ===
assert tools.add_ints(2, 3) == 5
assert t.get_list() == [1, 2, 3]
assert tools.return_value({'k': [1, 2]}) == {'k': [1, 2]}
assert tools.concat_strings('a', 'b') == 'ab'

# === names imported from the module ===
assert add_ints(1, 2) == 3
assert concat('x', 'y') == 'xy'
assert add_ints is tools.add_ints
assert t.add_ints is tools.add_ints

# === functions are values ===
fns = [tools.add_ints, tools.concat_strings]
assert [f(*a) for f, a in zip(fns, [(1, 1), ('p', 'q')])] == [2, 'pq']
assert hasattr(tools, 'add_ints')
assert not hasattr(tools, 'nope')

# === exceptions raised by a tool ===
try:
    tools.raise_error('ValueError', 'bad input')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'bad input'

# === a module the host does not have ===
try:
    import nope

    assert False, 'expected ModuleNotFoundError'
except ModuleNotFoundError as exc:
    assert str(exc) == "No module named 'nope'"

# === a name the module does not have ===
try:
    from tools import nope

    assert False, 'expected ImportError'
except ImportError as exc:
    assert str(exc) == "cannot import name 'nope' from 'tools' (unknown location)"


# === imports inside functions ===
def total():
    from tools import add_ints as plus

    return plus(plus(1, 2), 3)


assert total() == 6
