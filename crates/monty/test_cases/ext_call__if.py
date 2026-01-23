# call-external
# === External calls in if/else expressions ===

# Ternary expression with ext call in condition
result = 'yes' if add_ints(1, 1) == 2 else 'no'
assert result == 'yes', 'ext call in ternary condition true'

result2 = 'yes' if add_ints(1, 1) == 3 else 'no'
assert result2 == 'no', 'ext call in ternary condition false'

# Ext call in true branch
val = add_ints(10, 20) if True else 0
assert val == 30, 'ext call in ternary true branch'

# Ext call in false branch
val2 = 0 if False else add_ints(5, 5)
assert val2 == 10, 'ext call in ternary false branch'

# Ext calls in both branches
val3 = add_ints(1, 2) if True else add_ints(3, 4)
assert val3 == 3, 'ext call in both branches takes true'

val4 = add_ints(1, 2) if False else add_ints(3, 4)
assert val4 == 7, 'ext call in both branches takes false'

# === If statements with external calls ===

# Ext call in if condition
x = 0
if add_ints(1, 1) == 2:
    x = 100
assert x == 100, 'ext call in if statement condition true'

y = 0
if add_ints(1, 1) == 3:
    y = 100
assert y == 0, 'ext call in if statement condition false'

# Ext call in if body
z = 0
if True:
    z = add_ints(50, 50)
assert z == 100, 'ext call in if body'

# Ext call in else body
w = 0
if False:
    w = 1
else:
    w = add_ints(25, 75)
assert w == 100, 'ext call in else body'

# Nested ext calls in condition
nested = 0
if add_ints(add_ints(1, 2), add_ints(3, 4)) == 10:
    nested = 1
assert nested == 1, 'nested ext calls in if condition'

# Chained conditions with ext calls
result3 = 0
if add_ints(1, 1) == 2 and add_ints(2, 2) == 4:
    result3 = 1
assert result3 == 1, 'multiple ext calls in and condition'

result4 = 0
if add_ints(1, 1) == 3 or add_ints(2, 2) == 4:
    result4 = 1
assert result4 == 1, 'multiple ext calls in or condition'

# Comparison with ext call results
cmp = add_ints(10, 5) > add_ints(5, 5)
assert cmp == True, 'comparing two ext call results'

# === Nested if statements with external calls ===

# Nested if with ext calls in both conditions (both true)
result = 'none'
if return_value(1) == 1:
    if return_value(2) == 2:
        result = 'inner'
    else:
        result = 'outer_only'
else:
    result = 'failed'
assert result == 'inner', 'nested if both conditions true'

# Nested if - outer true, inner false
result2 = 'none'
if return_value(1) == 1:
    if return_value(2) == 999:
        result2 = 'inner'
    else:
        result2 = 'outer_only'
else:
    result2 = 'failed'
assert result2 == 'outer_only', 'nested if inner false'

# Nested if - outer false
result3 = 'none'
if return_value(1) == 999:
    if return_value(2) == 2:
        result3 = 'inner'
    else:
        result3 = 'outer_only'
else:
    result3 = 'xxx'
assert result3 == 'xxx', 'nested if outer false'

# Triple nested if - all true
result4 = 0
if return_value(1) == 1:
    if return_value(2) == 2:
        if return_value(3) == 3:
            result4 = 123
assert result4 == 123, 'triple nested if all true'

# If condition with multiple ext calls (addition)
result5 = 0
if add_ints(1, 2) + add_ints(3, 4) == 10:
    result5 = 1
assert result5 == 1, 'if condition with multiple ext calls'

# For loop inside if with external condition
total = 0
if return_value(1) == 1:
    for i in range(3):
        total = add_ints(total, return_value(i))
assert total == 3, 'for loop inside if with ext condition'

# For loop inside if - condition false
total2 = 0
if return_value(1) == 999:
    for i in range(3):
        total2 = add_ints(total2, i)
assert total2 == 0, 'for loop inside if condition false'
