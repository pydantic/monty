# === range() with one argument (stop) ===
assert list(range(0)) == [], 'range(0) is empty'
assert list(range(1)) == [0], 'range(1) is [0]'
assert list(range(5)) == [0, 1, 2, 3, 4], 'range(5) is [0, 1, 2, 3, 4]'
assert list(range(-3)) == [], 'range negative stop is empty'

# === range() with two arguments (start, stop) ===
assert list(range(0, 3)) == [0, 1, 2], 'range(0, 3)'
assert list(range(1, 5)) == [1, 2, 3, 4], 'range(1, 5)'
assert list(range(5, 10)) == [5, 6, 7, 8, 9], 'range(5, 10)'
assert list(range(3, 3)) == [], 'range equal start stop is empty'
assert list(range(5, 3)) == [], 'range start > stop is empty'
assert list(range(-5, -2)) == [-5, -4, -3], 'range negative to negative'
assert list(range(-3, 2)) == [-3, -2, -1, 0, 1], 'range negative to positive'

# === range() with three arguments (start, stop, step) ===
assert list(range(0, 10, 2)) == [0, 2, 4, 6, 8], 'range step 2'
assert list(range(1, 10, 3)) == [1, 4, 7], 'range step 3'
assert list(range(0, 10, 5)) == [0, 5], 'range step 5'
assert list(range(0, 10, 10)) == [0], 'range step equals diff'
assert list(range(0, 10, 20)) == [0], 'range step > diff'

# === range() with negative step ===
assert list(range(10, 0, -1)) == [10, 9, 8, 7, 6, 5, 4, 3, 2, 1], 'range step -1'
assert list(range(10, 0, -2)) == [10, 8, 6, 4, 2], 'range step -2'
assert list(range(5, 0, -1)) == [5, 4, 3, 2, 1], 'range 5 to 0 step -1'
assert list(range(0, 5, -1)) == [], 'range start < stop with negative step is empty'
assert list(range(-1, -5, -1)) == [-1, -2, -3, -4], 'range negative with negative step'

# === tuple(range()) conversions ===
assert tuple(range(3)) == (0, 1, 2), 'tuple(range(3))'
assert tuple(range(1, 4)) == (1, 2, 3), 'tuple(range(1, 4))'
assert tuple(range(0, 6, 2)) == (0, 2, 4), 'tuple(range(0, 6, 2))'

# === range in for loops ===
total = 0
for i in range(5):
    total = total + i
assert total == 10, 'for loop with range(5)'

total2 = 0
for i in range(1, 4):
    total2 = total2 + i
assert total2 == 6, 'for loop with range(1, 4)'

total3 = 0
for i in range(0, 10, 2):
    total3 = total3 + i
assert total3 == 20, 'for loop with range step 2'

# count down
countdown = []
for i in range(3, 0, -1):
    countdown.append(i)
assert countdown == [3, 2, 1], 'for loop countdown'

# === range repr ===
assert repr(range(5)) == 'range(0, 5)', 'repr range one arg'
assert repr(range(1, 5)) == 'range(1, 5)', 'repr range two args'
assert repr(range(1, 5, 2)) == 'range(1, 5, 2)', 'repr range three args'
assert repr(range(0, 10, 1)) == 'range(0, 10)', 'repr range step 1 omitted'
assert repr(range(5, 0, -1)) == 'range(5, 0, -1)', 'repr range negative step'

# === range type ===
assert type(range(5)) == range, 'type of range'
assert type(range(1, 5)) == range, 'type of range two args'
assert type(range(1, 5, 2)) == range, 'type of range three args'

# === range equality ===
assert range(5) == range(5), 'range equality same'
assert range(0, 5) == range(5), 'range(0, 5) == range(5)'
assert range(1, 5) == range(1, 5), 'range equality two args'
assert range(1, 5, 2) == range(1, 5, 2), 'range equality three args'
assert range(5) != range(6), 'range inequality'
assert range(1, 5) != range(2, 5), 'range inequality start differs'
assert range(1, 5, 1) != range(1, 5, 2), 'range inequality step differs'

# === range bool (truthiness) ===
assert bool(range(5)) == True, 'non-empty range is truthy'
assert bool(range(1, 5)) == True, 'range(1, 5) is truthy'
assert bool(range(0)) == False, 'empty range(0) is falsy'
assert bool(range(5, 5)) == False, 'empty range equal start stop is falsy'
assert bool(range(5, 0)) == False, 'empty range start > stop is falsy'
assert bool(range(5, 0, -1)) == True, 'range countdown is truthy'
assert bool(range(0, 5, -1)) == False, 'empty range wrong direction is falsy'

# === range isinstance ===
assert isinstance(range(5), range), 'isinstance range'

# === len(range()) ===
assert len(range(5)) == 5, 'len(range(5))'
assert len(range(0)) == 0, 'len(range(0))'
assert len(range(1, 5)) == 4, 'len(range(1, 5))'
assert len(range(0, 10, 2)) == 5, 'len(range step 2)'
assert len(range(10, 0, -1)) == 10, 'len(range negative step)'
assert len(range(0, 10, 3)) == 4, 'len(range step 3)'

# === range equality by sequence (not parameters) ===
assert range(0, 3, 2) == range(0, 4, 2), 'ranges with same sequence [0,2] are equal'
assert range(0, 5, 2) == range(0, 6, 2), 'range(0,5,2) == range(0,6,2) both [0,2,4]'
assert range(5, 0, -2) == range(5, -1, -2), 'negative step same sequence'
assert range(0) == range(0), 'empty ranges equal'
assert range(5, 5) == range(10, 10), 'different empty ranges equal'
assert range(0, 0) == range(5, 5), 'empty ranges with different params equal'

# === Range indexing (getitem) ===
# Basic indexing for range(stop)
r = range(5)
assert r[0] == 0, 'range(5)[0]'
assert r[1] == 1, 'range(5)[1]'
assert r[4] == 4, 'range(5)[4]'

# Negative indexing
assert r[-1] == 4, 'range(5)[-1]'
assert r[-2] == 3, 'range(5)[-2]'
assert r[-5] == 0, 'range(5)[-5]'

# Range with start
r = range(10, 15)
assert r[0] == 10, 'range(10, 15)[0]'
assert r[1] == 11, 'range(10, 15)[1]'
assert r[4] == 14, 'range(10, 15)[4]'
assert r[-1] == 14, 'range(10, 15)[-1]'
assert r[-5] == 10, 'range(10, 15)[-5]'

# Range with step
r = range(0, 10, 2)
assert r[0] == 0, 'range(0, 10, 2)[0]'
assert r[1] == 2, 'range(0, 10, 2)[1]'
assert r[2] == 4, 'range(0, 10, 2)[2]'
assert r[3] == 6, 'range(0, 10, 2)[3]'
assert r[4] == 8, 'range(0, 10, 2)[4]'
assert r[-1] == 8, 'range(0, 10, 2)[-1]'
assert r[-2] == 6, 'range(0, 10, 2)[-2]'

# Range with step 3
r = range(1, 10, 3)
assert r[0] == 1, 'range(1, 10, 3)[0]'
assert r[1] == 4, 'range(1, 10, 3)[1]'
assert r[2] == 7, 'range(1, 10, 3)[2]'
assert r[-1] == 7, 'range(1, 10, 3)[-1]'

# Range with negative step
r = range(10, 0, -1)
assert r[0] == 10, 'range(10, 0, -1)[0]'
assert r[1] == 9, 'range(10, 0, -1)[1]'
assert r[9] == 1, 'range(10, 0, -1)[9]'
assert r[-1] == 1, 'range(10, 0, -1)[-1]'
assert r[-10] == 10, 'range(10, 0, -1)[-10]'

# Range with negative step and larger step
r = range(10, 0, -2)
assert r[0] == 10, 'range(10, 0, -2)[0]'
assert r[1] == 8, 'range(10, 0, -2)[1]'
assert r[2] == 6, 'range(10, 0, -2)[2]'
assert r[3] == 4, 'range(10, 0, -2)[3]'
assert r[4] == 2, 'range(10, 0, -2)[4]'
assert r[-1] == 2, 'range(10, 0, -2)[-1]'

# Range starting from negative
r = range(-5, 0)
assert r[0] == -5, 'range(-5, 0)[0]'
assert r[2] == -3, 'range(-5, 0)[2]'
assert r[-1] == -1, 'range(-5, 0)[-1]'

# Single element range
r = range(42, 43)
assert r[0] == 42, 'single element range[0]'
assert r[-1] == 42, 'single element range[-1]'

# Variable index
r = range(100)
i = 50
assert r[i] == 50, 'range getitem with variable index'

# Bool indices (True=1, False=0)
r = range(10, 15)
assert r[False] == 10, 'range getitem with False'
assert r[True] == 11, 'range getitem with True'
