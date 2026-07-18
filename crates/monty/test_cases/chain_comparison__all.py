# === Basic chain comparisons ===
assert (1 < 2 < 3) == True
assert (1 < 3 < 2) == False
assert (3 < 2 < 1) == False
assert (1 <= 2 <= 2) == True
assert 1 <= 2 <= 2, 'with equality'
assert 1 <= 2 <= 2 <= 3, 'chained with equality'

# === Mixed operators ===
assert (1 < 2 <= 2 < 3) == True
assert (1 == 1 == 1) == True
assert (1 != 2 != 1) == True

# === Longer chains ===
assert (1 < 2 < 3 < 4 < 5) == True
assert (1 < 2 < 3 < 2 < 5) == False

# === With variables and expressions ===
x = 5
assert (1 < x < 10) == True
assert (0 < x - 3 < x < x + 1) == True


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
    assert result == False
    assert calls == ['a', 'b']


test_short_circuit()


# === Single evaluation of intermediate values ===
def test_single_eval():
    count = 0

    def middle():
        nonlocal count
        count += 1
        return 5

    result = 1 < middle() < 10
    assert result == True
    assert count == 1


test_single_eval()

# === Identity comparisons ===
a = [1]
b = a
c = a
assert (a is b is c) == True

# === Containment checks ===
assert (1 in [1, 2] in [[1, 2], [3]]) == True


# === Verify no namespace pollution ===
# Note: The old implementation used _chain_cmp_N variables which would leak.
# The new stack-based implementation doesn't create any intermediate variables.
# We can't easily test for namespace pollution without dir(), so we just verify
# that chain comparisons work correctly (covered by tests above).
