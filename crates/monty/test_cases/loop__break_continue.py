# === Basic break ===
result = []
for x in [1, 2, 3, 4, 5]:
    if x == 3:
        break
    result.append(x)
assert result == [1, 2], 'break exits loop early'

# === Break skips else ===
flag = 0
for x in [1, 2, 3]:
    if x == 2:
        break
else:
    flag = 1
assert flag == 0, 'break skips else clause'

# === No break runs else ===
flag = 0
for x in [1, 2, 3]:
    pass
else:
    flag = 1
assert flag == 1, 'completing loop runs else clause'

# === Basic continue ===
result = []
for x in [1, 2, 3, 4, 5]:
    if x % 2 == 0:
        continue
    result.append(x)
assert result == [1, 3, 5], 'continue skips iteration'

# === Continue with else ===
flag = 0
for x in [1, 2, 3]:
    if x == 2:
        continue
else:
    flag = 1
assert flag == 1, 'continue does not skip else clause'

# === Nested loops - break inner ===
result = []
for i in [1, 2, 3]:
    for j in ['a', 'b', 'c']:
        if j == 'b':
            break
        result.append((i, j))
assert result == [(1, 'a'), (2, 'a'), (3, 'a')], 'break only affects inner loop'

# === Nested loops - continue inner ===
result = []
for i in [1, 2]:
    for j in ['a', 'b', 'c']:
        if j == 'b':
            continue
        result.append((i, j))
assert result == [(1, 'a'), (1, 'c'), (2, 'a'), (2, 'c')], 'continue only affects inner loop'

# === Break in nested with else on inner ===
result = []
for i in [1, 2]:
    for j in [10, 20, 30]:
        if j == 20:
            break
        result.append(j)
    else:
        result.append('inner-else')
assert result == [10, 10], 'break skips inner else'

# === No break in inner runs inner else ===
result = []
for i in [1, 2]:
    for j in [10, 20]:
        result.append(j)
    else:
        result.append('inner-else')
assert result == [10, 20, 'inner-else', 10, 20, 'inner-else'], 'no break runs inner else'

# === Continue does not affect else ===
result = []
for x in [1, 2, 3]:
    if x == 2:
        continue
    result.append(x)
else:
    result.append('else')
assert result == [1, 3, 'else'], 'continue does not prevent else'

# === Empty loop with else ===
flag = 0
for x in []:
    flag = 1
else:
    flag = 2
assert flag == 2, 'empty loop runs else'

# === Break on first iteration ===
result = []
for x in [1, 2, 3]:
    result.append('before')
    break
    result.append('after')  # unreachable
assert result == ['before'], 'break on first iteration'

# === Continue on every iteration ===
result = []
for x in [1, 2, 3]:
    result.append(x)
    continue
    result.append('after')  # unreachable
assert result == [1, 2, 3], 'continue on every iteration'
