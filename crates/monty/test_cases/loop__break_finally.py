# === Break in try/finally must run finally ===
result = []
for x in [1, 2, 3]:
    try:
        result.append('before')
        break
        result.append('after')  # unreachable
    finally:
        result.append('finally')
assert result == ['before', 'finally'], f'break in try/finally should run finally: {result}'

# === Break in nested try/finally runs both finally blocks ===
result = []
for x in [1, 2, 3]:
    try:
        try:
            result.append('inner-try')
            break
        finally:
            result.append('inner-finally')
    finally:
        result.append('outer-finally')
assert result == ['inner-try', 'inner-finally', 'outer-finally'], f'nested finally blocks: {result}'

# === Break in try/except/finally runs finally ===
result = []
for x in [1, 2, 3]:
    try:
        result.append('try')
        break
    except ValueError:
        result.append('except')
    finally:
        result.append('finally')
assert result == ['try', 'finally'], f'break in try/except/finally: {result}'

# === Break inside except handler with finally ===
result = []
for x in [1, 2, 3]:
    try:
        raise ValueError('test')
    except ValueError:
        result.append('except')
        break
    finally:
        result.append('finally')
assert result == ['except', 'finally'], f'break in except with finally: {result}'

# === Break does not run finally if not in try ===
result = []
for x in [1, 2, 3]:
    result.append('body')
    break
assert result == ['body'], f'break without finally: {result}'

# === Break with multiple loops and finally ===
result = []
for i in [1, 2]:
    try:
        for j in [10, 20, 30]:
            if j == 20:
                break  # This break should not trigger outer finally
            result.append(j)
        result.append('after-inner')
    finally:
        result.append('outer-finally')
assert result == [10, 'after-inner', 'outer-finally', 10, 'after-inner', 'outer-finally'], (
    f'inner break with outer finally: {result}'
)
