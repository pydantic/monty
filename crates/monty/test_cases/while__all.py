# === Basic while loop ===
i = 0
result = []
while i < 3:
    result.append(i)
    i += 1
assert result == [0, 1, 2], 'basic while loop'

# === While with break ===
i = 0
result = []
while i < 10:
    if i == 3:
        break
    result.append(i)
    i += 1
assert result == [0, 1, 2], 'while with break'

# === While with continue ===
i = 0
result = []
while i < 5:
    i += 1
    if i % 2 == 0:
        continue
    result.append(i)
assert result == [1, 3, 5], 'while with continue'

# === While with else (no break - else runs) ===
i = 0
flag = 0
while i < 3:
    i += 1
else:
    flag = 1
assert flag == 1, 'while else runs when no break'

# === While with else (with break - else skipped) ===
i = 0
flag = 0
while i < 10:
    i += 1
    if i == 2:
        break
else:
    flag = 1
assert flag == 0, 'while else skipped on break'

# === while True with break ===
i = 0
result = []
while True:
    result.append(i)
    i += 1
    if i >= 3:
        break
assert result == [0, 1, 2], 'while True with break'

# === while False (never executes) ===
flag = 0
while False:
    flag = 1
assert flag == 0, 'while False never executes'

# === while False with else (else runs immediately) ===
flag = 0
while False:
    flag = 1
else:
    flag = 2
assert flag == 2, 'while False runs else immediately'

# === Nested while loops ===
i = 0
result = []
while i < 2:
    j = 0
    while j < 2:
        result.append((i, j))
        j += 1
    i += 1
assert result == [(0, 0), (0, 1), (1, 0), (1, 1)], 'nested while loops'

# === Nested while with break inner ===
i = 0
result = []
while i < 3:
    j = 0
    while j < 3:
        if j == 1:
            break
        result.append((i, j))
        j += 1
    i += 1
assert result == [(0, 0), (1, 0), (2, 0)], 'nested while break inner only'

# === For inside while ===
i = 0
result = []
while i < 2:
    for j in ['a', 'b']:
        result.append((i, j))
    i += 1
assert result == [(0, 'a'), (0, 'b'), (1, 'a'), (1, 'b')], 'for inside while'

# === While inside for ===
result = []
for i in [0, 1]:
    j = 0
    while j < 2:
        result.append((i, j))
        j += 1
assert result == [(0, 0), (0, 1), (1, 0), (1, 1)], 'while inside for'

# === Complex condition with and ===
i = 0
j = 10
result = []
while i < 5 and j > 5:
    result.append((i, j))
    i += 1
    j -= 1
assert result == [(0, 10), (1, 9), (2, 8), (3, 7), (4, 6)], 'while with and condition'

# === Complex condition with or ===
i = 5
count = 0
while i < 3 or count < 2:
    count += 1
    i += 1
assert count == 2, 'while with or condition'


# === While with function call condition ===
def check(n):
    return n < 3


i = 0
result = []
while check(i):
    result.append(i)
    i += 1
assert result == [0, 1, 2], 'while with function call condition'

# === Continue does not skip else ===
i = 0
flag = 0
while i < 3:
    i += 1
    if i == 2:
        continue
else:
    flag = 1
assert flag == 1, 'continue does not skip else'

# === Nested while - break outer via flag ===
i = 0
result = []
done = False
while i < 3 and not done:
    j = 0
    while j < 3:
        if i == 1 and j == 1:
            done = True
            break
        result.append((i, j))
        j += 1
    i += 1
assert result == [(0, 0), (0, 1), (0, 2), (1, 0)], 'nested while with flag break'

# === While with negative condition ===
i = 5
result = []
while not i == 3:
    result.append(i)
    i -= 1
assert result == [5, 4], 'while with not condition'

# === Nested while with inner else ===
i = 0
result = []
while i < 2:
    j = 0
    while j < 2:
        result.append(j)
        j += 1
    else:
        result.append('inner-else')
    i += 1
assert result == [0, 1, 'inner-else', 0, 1, 'inner-else'], 'nested while with inner else'

# === Break in nested while skips inner else ===
i = 0
result = []
while i < 2:
    j = 0
    while j < 3:
        if j == 1:
            break
        result.append(j)
        j += 1
    else:
        result.append('inner-else')
    i += 1
assert result == [0, 0], 'break skips inner else only'
