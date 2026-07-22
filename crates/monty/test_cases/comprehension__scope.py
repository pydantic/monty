# === Target absence in globals() at module scope ===
[x for x in range(10)]

try:
    x
    assert False, "Expected NameError for 'x' after comprehension"
except NameError:
    pass

# === Outer variable preservation ===
outer_x = 5
[outer_x for outer_x in [1, 2, 3]]
assert outer_x == 5

# === Walrus binds in enclosing scope ===
# PEP 572: walrus targets are assigned in the enclosing scope, even when written
# inside a comprehension.
walrus_result = [(walrus_y := walrus_w) for walrus_w in [1, 2, 3]]
assert walrus_result == [1, 2, 3]
assert walrus_y == 3

# === Nested same-name targets simultaneously live ===
matrix = [[1, 2], [3, 4]]
nested = [[inner_x * 10 for inner_x in inner_x] for inner_x in matrix]
assert nested == [[10, 20], [30, 40]]

# === Sibling comps reusing the same comp-var stack slot ===
# Each RaiseUnboundLocal carries its own name in the opcode, so an unbound-name
# error in the second sibling must report the second sibling's target name —
# not whichever name the compiler emitted first for the shared slot.
[first_name for first_name in [1, 2]]
try:
    [_s for _s in [1] for _ignored in [second_late] for second_late in [[2]]]
    assert False, 'expected UnboundLocalError from second sibling comp'
except UnboundLocalError as exc:
    assert str(exc) == "cannot access local variable 'second_late' where it is not associated with a value"

# === Comprehension targets captured by closures ===
funcs = [lambda: item for item in ['first', 'second']]
assert funcs[0]() == 'second'
assert funcs[1]() == 'second'

nested_lambda_funcs = [lambda: lambda: item for item in ['first', 'second']]
assert nested_lambda_funcs[0]()() == 'second'
assert {(lambda: item)() for item in [1, 2]} == {1, 2}


def make_comprehension_closure():
    item = 'outer'
    return [lambda: item for item in ['inner']][0]


assert make_comprehension_closure()() == 'inner'
assert [(lambda: item)() for item in [1, 2]] == [1, 2]

pair_funcs = [lambda: (left, right) for left, right in [(1, 2), (3, 4)]]
assert pair_funcs[0]() == (3, 4)
assert pair_funcs[1]() == (3, 4)

# Repeated targets in one comprehension bind the same lexical name. Later
# iterables observe the preceding binding, and closures share its stable cell.
assert [repeated for repeated in [1, 2] for repeated in [repeated + 10]] == [11, 12]
repeated_funcs = [lambda: repeated for repeated in [1, 2] for repeated in [repeated]]
assert repeated_funcs[0]() == 2
assert repeated_funcs[1]() == 2

repeated_unpack_funcs = [lambda: repeated for repeated, repeated in [(1, 2)]]
assert repeated_unpack_funcs[0]() == 2

nested_funcs = [[lambda: item for item in row] for item, row in [('outer', ['a', 'b'])]]
assert nested_funcs[0][0]() == 'b'

# === Nested-tuple comp target (exercises LiftToTop) ===
# Flat parts of the tuple stay at their UNPACK position; the inner (b, c)
# requires a Lift to bring it to TOS for further unpacking. The compiler
# tracks each leaf's final operand-stack offset.
items = [(1, (2, 3), 4), (5, (6, 7), 8)]
flat_nested = [a + b + c + d for a, (b, c), d in items]
assert flat_nested == [10, 26]

# Doubly-nested tuple
nested_pairs = [((1, 2), (3, 4))]
doubly_nested = [(a, b, c, d) for (a, b), (c, d) in nested_pairs]
assert doubly_nested == [(1, 2, 3, 4)]

# Starred sub-target in a comp
starred = [(a, rest, last) for a, *rest, last in [(1, 2, 3, 4, 5)]]
assert starred == [(1, [2, 3, 4], 5)]
