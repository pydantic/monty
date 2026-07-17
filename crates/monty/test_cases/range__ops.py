# === range() with one argument (stop) ===
assert list(range(0)) == []
assert list(range(1)) == [0]
assert list(range(5)) == [0, 1, 2, 3, 4]
assert list(range(-3)) == []

# === range() with two arguments (start, stop) ===
assert list(range(0, 3)) == [0, 1, 2]
assert list(range(1, 5)) == [1, 2, 3, 4]
assert list(range(5, 10)) == [5, 6, 7, 8, 9]
assert list(range(3, 3)) == []
assert list(range(5, 3)) == []
assert list(range(-5, -2)) == [-5, -4, -3]
assert list(range(-3, 2)) == [-3, -2, -1, 0, 1]

# === range() with three arguments (start, stop, step) ===
assert list(range(0, 10, 2)) == [0, 2, 4, 6, 8]
assert list(range(1, 10, 3)) == [1, 4, 7]
assert list(range(0, 10, 5)) == [0, 5]
assert list(range(0, 10, 10)) == [0]
assert list(range(0, 10, 20)) == [0]

# === range() with negative step ===
assert list(range(10, 0, -1)) == [10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
assert list(range(10, 0, -2)) == [10, 8, 6, 4, 2]
assert list(range(5, 0, -1)) == [5, 4, 3, 2, 1]
assert list(range(0, 5, -1)) == []
assert list(range(-1, -5, -1)) == [-1, -2, -3, -4]

# === tuple(range()) conversions ===
assert tuple(range(3)) == (0, 1, 2)
assert tuple(range(1, 4)) == (1, 2, 3)
assert tuple(range(0, 6, 2)) == (0, 2, 4)

# === range in for loops ===
total = 0
for i in range(5):
    total = total + i
assert total == 10

total2 = 0
for i in range(1, 4):
    total2 = total2 + i
assert total2 == 6

total3 = 0
for i in range(0, 10, 2):
    total3 = total3 + i
assert total3 == 20

# count down
countdown = []
for i in range(3, 0, -1):
    countdown.append(i)
assert countdown == [3, 2, 1]

# === range repr ===
assert repr(range(5)) == 'range(0, 5)'
assert repr(range(1, 5)) == 'range(1, 5)'
assert repr(range(1, 5, 2)) == 'range(1, 5, 2)'
assert repr(range(0, 10, 1)) == 'range(0, 10)'
assert repr(range(5, 0, -1)) == 'range(5, 0, -1)'

# === range type ===
assert type(range(5)) == range
assert type(range(1, 5)) == range
assert type(range(1, 5, 2)) == range

# === range equality ===
assert range(5) == range(5)
assert range(0, 5) == range(5)
assert range(1, 5) == range(1, 5)
assert range(1, 5, 2) == range(1, 5, 2)
assert range(5) != range(6)
assert range(1, 5) != range(2, 5)
assert range(1, 5, 1) != range(1, 5, 2)

# === range bool (truthiness) ===
assert bool(range(5)) == True
assert bool(range(1, 5)) == True
assert bool(range(0)) == False
assert bool(range(5, 5)) == False
assert bool(range(5, 0)) == False
assert bool(range(5, 0, -1)) == True
assert bool(range(0, 5, -1)) == False

# === range isinstance ===
assert isinstance(range(5), range)

# === len(range()) ===
assert len(range(5)) == 5
assert len(range(0)) == 0
assert len(range(1, 5)) == 4
assert len(range(0, 10, 2)) == 5
assert len(range(10, 0, -1)) == 10
assert len(range(0, 10, 3)) == 4
assert len(range(-(2**62), 2**62 - 1)) == 2**63 - 1
try:
    assert len(range(-(2**62), (2**62))) == 2**63
    assert False, 'len(range with bounds at int64 limits should raise OverflowError'
except OverflowError as e:
    assert str(e) == 'Python int too large to convert to C ssize_t', str(e)

# === range equality by sequence (not parameters) ===
assert range(0, 3, 2) == range(0, 4, 2)
assert range(0, 5, 2) == range(0, 6, 2)
assert range(5, 0, -2) == range(5, -1, -2)
assert range(0) == range(0)
assert range(5, 5) == range(10, 10)
assert range(0, 0) == range(5, 5)

# === Range indexing (getitem) ===
# Basic indexing for range(stop)
r = range(5)
assert r[0] == 0
assert r[1] == 1
assert r[4] == 4

# Negative indexing
assert r[-1] == 4
assert r[-2] == 3
assert r[-5] == 0

# Range with start
r = range(10, 15)
assert r[0] == 10
assert r[1] == 11
assert r[4] == 14
assert r[-1] == 14
assert r[-5] == 10

# Range with step
r = range(0, 10, 2)
assert r[0] == 0
assert r[1] == 2
assert r[2] == 4
assert r[3] == 6
assert r[4] == 8
assert r[-1] == 8
assert r[-2] == 6

# Range with step 3
r = range(1, 10, 3)
assert r[0] == 1
assert r[1] == 4
assert r[2] == 7
assert r[-1] == 7

# Range with negative step
r = range(10, 0, -1)
assert r[0] == 10
assert r[1] == 9
assert r[9] == 1
assert r[-1] == 1
assert r[-10] == 10

# Range with negative step and larger step
r = range(10, 0, -2)
assert r[0] == 10
assert r[1] == 8
assert r[2] == 6
assert r[3] == 4
assert r[4] == 2
assert r[-1] == 2

# Range starting from negative
r = range(-5, 0)
assert r[0] == -5
assert r[2] == -3
assert r[-1] == -1

# Single element range
r = range(42, 43)
assert r[0] == 42
assert r[-1] == 42

# Variable index
r = range(100)
i = 50
assert r[i] == 50

# Bool indices (True=1, False=0)
r = range(10, 15)
assert r[False] == 10
assert r[True] == 11

# === Range containment ('in' operator) ===
# Basic containment
assert 0 in range(5)
assert 4 in range(5)
assert 5 not in range(5)
assert -1 not in range(5)

# Range with start
assert 10 in range(10, 15)
assert 14 in range(10, 15)
assert 15 not in range(10, 15)
assert 9 not in range(10, 15)

# Range with step
assert 0 in range(0, 10, 2)
assert 2 in range(0, 10, 2)
assert 8 in range(0, 10, 2)
assert 3 not in range(0, 10, 2)
assert 10 not in range(0, 10, 2)

# Range with negative step
assert 10 in range(10, 0, -1)
assert 1 in range(10, 0, -1)
assert 0 not in range(10, 0, -1)
assert 11 not in range(10, 0, -1)

# Negative step with step > 1
assert 10 in range(10, 0, -2)
assert 8 in range(10, 0, -2)
assert 9 not in range(10, 0, -2)

# Negative ranges
assert -3 in range(-5, 0)
assert 0 not in range(-5, 0)

# Empty ranges
assert 5 not in range(0)
assert 0 not in range(5, 5)

# Non-int types return False (no TypeError)
assert 'a' not in range(5)

# Float containment (floats equal to integers are contained)
assert 3.0 in range(5)
assert 0.0 in range(5)
assert 4.0 in range(5)
assert 3.5 not in range(5)
assert 5.0 not in range(5)
assert 2.0 in range(0, 10, 2)
assert 3.0 not in range(0, 10, 2)
assert -1.0 not in range(5)

# Bool as container element (True=1, False=0 for comparison)
assert True in range(5)
assert False in range(5)
assert True not in range(0)

# Large ranges which can hit monty's range i64 limits should not panic
assert range(-(2**63), 2**63 - 1)[0] == -(2**63)
assert range(-(2**63), 2**63 - 1, 2**63 - 1)[2] == 2**63 - 2

# === Check that containment doesn't overflow i64 calculation ===
assert 100 in range(-(2**63), 2**63 - 1, 3)
assert 101 not in range(-(2**63), 2**63 - 1, 3)
assert -(2**63) in range(-(2**63), 2**63 - 1, 1)
assert (2**63 - 2) in range(-(2**63), 2**63 - 1, 1)
assert (2**63 - 1) not in range(-(2**63), 2**63 - 1, 1)
assert -1 in range(2**63 - 1, -(2**63), -1)
assert (2**63 - 1) in range(2**63 - 1, -(2**63), -1)
assert -(2**63) not in range(2**63 - 1, -(2**63), -1)

# === Equality: ranges compare by the sequence they produce ===
assert range(0, 3) == range(0, 3)
assert range(0, 3) != range(0, 4)
# Empty ranges are equal regardless of start/stop/step
assert range(0, 0) == range(5, 5)
assert range(0, 0, 1) == range(10, 5, 3)
# Single-element ranges are equal when their one element matches, regardless of step
assert range(0, 1, 1) == range(0, 2, 2)
assert range(5, 6) == range(5, 7, 100)
assert range(0, 1) != range(1, 2)
# Multi-element ranges must match start and step (stop may differ if sequence matches)
assert range(0, 4, 2) == range(0, 3, 2)
assert range(0, 4, 2) != range(0, 4, 1)

# === Hash consistency: equal ranges must hash equally (dict-key invariant) ===
assert hash(range(0, 1, 1)) == hash(range(0, 2, 2))
assert hash(range(0, 0)) == hash(range(5, 5))
assert hash(range(0, 4, 2)) == hash(range(0, 3, 2))
assert {range(0, 1, 1): 'a'}[range(0, 2, 2)] == 'a'
