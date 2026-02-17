# === Basic chain comparisons ===
assert (1 < 2 < 3) == True, 'ascending chain'
assert (1 < 3 < 2) == False, 'fails at second comparison'
assert (3 < 2 < 1) == False, 'fails at first comparison'
assert (1 <= 2 <= 2) == True, 'with equality'
assert 1 <= 2 <= 2, 'with equality'
assert 1 <= 2 <= 2 <= 3, 'chained with equality'

# === Mixed operators ===
assert (1 < 2 <= 2 < 3) == True, 'mixed lt and le'
assert (1 == 1 == 1) == True, 'triple equality'
assert (1 != 2 != 1) == True, 'not-equal chain (not transitive)'

# === Longer chains ===
assert (1 < 2 < 3 < 4 < 5) == True, '5-way ascending'
assert (1 < 2 < 3 < 2 < 5) == False, 'fails in middle'

# === With variables and expressions ===
x = 5
assert (1 < x < 10) == True, 'variable in chain'
assert (0 < x - 3 < x < x + 1) == True, 'expressions'


# === Short-circuit evaluation ===
def test_short_circuit():
    calls = []

    def a():
        calls.append('a')
        return 1

    def b():
        calls.append('b')
        return 0  # This will make first comparison fail

    def c():
        calls.append('c')
        return 2

    # Test: first comparison fails, c() should not be called
    result = a() < b() < c()  # 1 < 0 is False, c() should not be called
    assert result == False, 'short circuit result'
    assert calls == ['a', 'b'], 'c not called due to short circuit'


test_short_circuit()


# === Single evaluation of intermediate values ===
def test_single_eval():
    count = 0

    def middle():
        nonlocal count
        count += 1
        return 5

    result = 1 < middle() < 10
    assert result == True, 'chain result'
    assert count == 1, 'middle() called exactly once'


test_single_eval()

# === Identity comparisons ===
a = [1]
b = a
c = a
assert (a is b is c) == True, 'is chain same object'

# === Containment checks ===
assert (1 in [1, 2] in [[1, 2], [3]]) == True, 'in chain'


# === Verify no namespace pollution ===
# Note: The old implementation used _chain_cmp_N variables which would leak.
# The new stack-based implementation doesn't create any intermediate variables.
# We can't easily test for namespace pollution without dir(), so we just verify
# that chain comparisons work correctly (covered by tests above).
