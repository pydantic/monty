# === Continue in try/finally must run finally ===
result = []
for x in [1, 2, 3]:
    try:
        result.append(x)
        if x == 2:
            continue
        result.append('after-continue')
    finally:
        result.append('finally')
assert result == [1, 'after-continue', 'finally', 2, 'finally', 3, 'after-continue', 'finally'], (
    f'continue in try/finally should run finally: {result}'
)

# === Continue in nested try/finally runs both finally blocks ===
result = []
for x in [1, 2]:
    try:
        try:
            result.append(x)
            continue
        finally:
            result.append('inner-finally')
    finally:
        result.append('outer-finally')
assert result == [1, 'inner-finally', 'outer-finally', 2, 'inner-finally', 'outer-finally'], (
    f'nested finally with continue: {result}'
)

# === Continue in try/except/finally runs finally ===
result = []
for x in [1, 2, 3]:
    try:
        result.append(x)
        if x == 2:
            continue
    except ValueError:
        result.append('except')
    finally:
        result.append('finally')
assert result == [1, 'finally', 2, 'finally', 3, 'finally'], f'continue in try/except/finally: {result}'

# === Continue inside except handler with finally ===
result = []
for x in [1, 2, 3]:
    try:
        if x == 2:
            raise ValueError('test')
        result.append(x)
    except ValueError:
        result.append('except')
        continue
    finally:
        result.append('finally')
    result.append('after')
assert result == [1, 'finally', 'after', 'except', 'finally', 3, 'finally', 'after'], (
    f'continue in except with finally: {result}'
)

# === Continue does not run finally if not in try ===
result = []
for x in [1, 2, 3]:
    result.append(x)
    continue
    result.append('unreachable')
assert result == [1, 2, 3], f'continue without finally: {result}'

# === Continue with multiple loops and finally ===
result = []
for i in [1, 2]:
    try:
        for j in [10, 20, 30]:
            if j == 20:
                continue  # This continue should not trigger outer finally
            result.append(j)
        result.append('after-inner')
    finally:
        result.append('outer-finally')
assert result == [10, 30, 'after-inner', 'outer-finally', 10, 30, 'after-inner', 'outer-finally'], (
    f'inner continue with outer finally: {result}'
)
