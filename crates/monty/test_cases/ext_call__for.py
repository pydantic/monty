# call-external
# === External calls in for loops ===

# Ext call in loop body
total = 0
for i in range(3):
    total = add_ints(total, 1)
assert total == 3, f'ext call accumulator in loop, {total=}'

# Ext call with loop variable
sum_val = 0
for i in range(4):
    sum_val = add_ints(sum_val, i)
assert sum_val == 6, 'ext call with loop var'

# Multiple ext calls per iteration
result = 0
for i in range(3):
    result = add_ints(result, add_ints(i, i))
assert result == 6, 'nested ext calls in loop'

# Building list with ext calls
items = []
for i in range(3):
    items.append(add_ints(i, 10))
assert items[0] == 10, 'ext call list build first'
assert items[1] == 11, 'ext call list build second'
assert items[2] == 12, 'ext call list build third'

# Chained ext calls in loop
acc = 0
for i in range(3):
    acc = add_ints(acc, 1) + add_ints(0, 1)
assert acc == 6, 'chained ext calls in loop body'

# Nested loops with ext calls
matrix_sum = 0
for i in range(2):
    for j in range(2):
        matrix_sum = add_ints(matrix_sum, add_ints(i, j))
assert matrix_sum == 4, 'ext calls in nested loops'

# === More nested loop edge cases ===

# Nested loops building a result list
results = []
for i in range(2):
    for j in range(2):
        results.append(concat_strings(return_value(str(i)), return_value(str(j))))
assert results[0] == '00', 'nested loops result list first'
assert results[1] == '01', 'nested loops result list second'
assert results[2] == '10', 'nested loops result list third'
assert results[3] == '11', 'nested loops result list fourth'

# If inside for loop with external call condition
filtered = []
for i in range(3):
    if return_value(i) == i:
        filtered.append(i)
assert filtered == [0, 1, 2], 'if inside for with ext condition'

# If inside for loop - some iterations match
results2 = []
for i in range(4):
    # Only append even numbers (using modulo check with add_ints)
    if add_ints(i, 0) % 2 == 0:
        results2.append(return_value(i))
assert results2 == [0, 2], 'if inside for filtering with ext'

# Nested for with different ranges
outer_sum = 0
for i in range(2):
    inner_sum = 0
    for j in range(3):
        inner_sum = add_ints(inner_sum, add_ints(i, j))
    outer_sum = add_ints(outer_sum, inner_sum)
# i=0: (0+0)+(0+1)+(0+2) = 0+1+2 = 3
# i=1: (1+0)+(1+1)+(1+2) = 1+2+3 = 6
# total = 3 + 6 = 9
assert outer_sum == 9, 'nested for with accumulator'

# Three levels of nested loops
count = 0
for i in range(2):
    for j in range(2):
        for k in range(2):
            count = add_ints(count, 1)
assert count == 8, 'triple nested for with ext call'

# multiple ext calls in iterable

ext_ints = []
for i in add_ints(1, 1), add_ints(2, 2), add_ints(3, 3):
    ext_ints.append(i)
assert ext_ints == [2, 4, 6], 'multiple ext calls in iterable'

# ext call iterable, get_list() returns [1, 2, 3]
total = 0
for x in get_list():
    total = add_ints(total, x)
assert total == 6, 'ext call iterable'

# string iteration with ext call in body
chars = []
for c in 'abc':
    chars.append(return_value(c))
assert chars == ['a', 'b', 'c'], f'string iteration with ext call: {chars}'

# unicode string iteration with ext call in body
# Tests decr() handling of multi-byte UTF-8 characters (1-4 bytes each)
unicode_chars = []
for c in 'aé中😀b':  # a (1 byte), e-acute (2), chinese (3), emoji (4), b (1)
    unicode_chars.append(return_value(c))
assert unicode_chars == ['a', 'é', '中', '😀', 'b'], f'unicode iteration: {unicode_chars}'
