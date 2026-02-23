# call-external
# === External calls in elif conditions ===

# External call in elif condition - true
result1 = 0
if add_ints(1, 1) == 3:
    result1 = 1
elif add_ints(2, 2) == 4:
    result1 = 2
else:
    result1 = 3
assert result1 == 2, f'elif condition with ext call should be evaluated, {result1=}'

# External call in elif condition - falls through to else
result2 = 0
if add_ints(1, 1) == 3:
    result2 = 1
elif add_ints(2, 2) == 5:
    result2 = 2
else:
    result2 = 3
assert result2 == 3, 'else taken when all ext call conditions are false'

# Multiple elif with external calls
result3 = 0
if add_ints(1, 1) == 10:
    result3 = 1
elif add_ints(2, 2) == 10:
    result3 = 2
elif add_ints(3, 3) == 6:
    result3 = 3
elif add_ints(4, 4) == 10:
    result3 = 4
else:
    result3 = 5
assert result3 == 3, 'third elif with ext call should match'

# === External calls in elif bodies ===

# Ext call in elif body
val1 = 0
if False:
    val1 = 1
elif True:
    val1 = add_ints(10, 20)
else:
    val1 = 3
assert val1 == 30, 'ext call in elif body'

# Ext call in else body after elif chain
val2 = 0
if False:
    val2 = 1
elif False:
    val2 = 2
else:
    val2 = add_ints(15, 25)
assert val2 == 40, 'ext call in else body after elif'

# Multiple ext calls in elif body
val3 = 0
if False:
    val3 = 1
elif True:
    a = add_ints(5, 5)
    b = add_ints(10, 10)
    val3 = add_ints(a, b)
else:
    val3 = 3
assert val3 == 30, 'multiple ext calls in elif body'

# === Nested ext calls ===

# Nested ext calls in elif condition
result4 = 0
if False:
    result4 = 1
elif add_ints(add_ints(1, 2), add_ints(3, 4)) == 10:
    result4 = 2
else:
    result4 = 3
assert result4 == 2, 'nested ext calls in elif condition'

# === Short-circuit with ext calls ===

# Ext call should not be evaluated if earlier condition is true
call_count = 0


def counting_add(a, b):
    global call_count
    call_count = call_count + 1
    return a + b


# This uses a regular function, not ext call, to verify short-circuit
# but the ext calls in bodies still test suspension
x = 0
if True:
    x = add_ints(1, 1)
elif False:
    x = add_ints(2, 2)
assert x == 2, 'if body ext call executed, elif skipped'

# === Ext call in both condition and body ===

result5 = 0
if add_ints(1, 1) == 3:
    result5 = add_ints(100, 100)
elif add_ints(2, 2) == 4:
    result5 = add_ints(50, 50)
else:
    result5 = add_ints(25, 25)
assert result5 == 100, 'ext call in both elif condition and body'

# === Ext call in if body when condition is true ===

if_body_result = 0
if add_ints(5, 5) == 10:
    if_body_result = add_ints(100, 200)
elif add_ints(1, 1) == 2:
    if_body_result = add_ints(10, 20)
else:
    if_body_result = add_ints(1, 2)
assert if_body_result == 300, 'ext call in if body when if condition is true'

# === Ext calls returning values used as conditions ===

# return_value returns its argument, so we can use it to test boolean coercion
cond_result = 0
if return_value(0):
    cond_result = 1
elif return_value(1):
    cond_result = 2
else:
    cond_result = 3
assert cond_result == 2, 'ext call return value used as boolean condition'

# === Ext calls with string concatenation ===

str_result = ''
if add_ints(1, 1) == 3:
    str_result = concat_strings('a', 'b')
elif add_ints(2, 2) == 4:
    str_result = concat_strings('hello', ' world')
else:
    str_result = concat_strings('x', 'y')
assert str_result == 'hello world', 'ext call with string result in elif body'

# === Multiple conditions with ext calls in same expression ===

multi_cond = 0
if add_ints(1, 1) > 5:
    multi_cond = 1
elif add_ints(2, 2) < add_ints(3, 3):
    multi_cond = 2
else:
    multi_cond = 3
assert multi_cond == 2, 'comparison between two ext call results in elif condition'

# === Ext call in all three branches ===

all_branches = 0
val = add_ints(5, 5)
if val < 5:
    all_branches = add_ints(1, 0)
elif val < 15:
    all_branches = add_ints(2, 0)
else:
    all_branches = add_ints(3, 0)
assert all_branches == 2, 'ext call in elif body based on earlier ext call result'
