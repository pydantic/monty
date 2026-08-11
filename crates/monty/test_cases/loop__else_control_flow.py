# Control flow statements inside loop `else` clauses. The else block is
# compiled after the loop's own block is closed, so break/continue/return
# there must target the *enclosing* loop (or function), never the loop the
# else belongs to.

# === classic search idiom: else-continue + break targets the outer loop ===
log = []
for i in range(3):
    for j in range(3):
        if i + j == 3:
            log.append(f'hit {i} {j}')
            break
    else:
        log.append(f'exhausted {i}')
        continue
    break
else:
    log.append('outer exhausted')
assert log == ['exhausted 0', 'hit 1 2']

# === outer else runs when the inner search never breaks the outer loop ===
log = []
for i in range(2):
    for j in range(2):
        if i + j == 99:
            break
    else:
        continue
    break
else:
    log.append('outer exhausted')
assert log == ['outer exhausted']

# === break in while-else exits the enclosing while ===
log = []
outer = 0
while outer < 3:
    outer += 1
    while False:
        pass
    else:
        log.append(f'inner-else {outer}')
        if outer == 2:
            break
log.append('done')
assert log == ['inner-else 1', 'inner-else 2', 'done']

# === continue in while-else continues the enclosing loop ===
log = []
for i in range(3):
    while False:
        pass
    else:
        log.append(i)
        continue
    log.append('unreached')
assert log == [0, 1, 2]


# === return in for-else ===
def search(needle, haystack):
    for item in haystack:
        if item == needle:
            return 'found'
    else:
        return 'missing'


assert search(2, [1, 2, 3]) == 'found'
assert search(9, [1, 2, 3]) == 'missing'


# === return in while-else ===
def count_down(n):
    while n > 0:
        n -= 1
        if n == 99:
            return 'impossible'
    else:
        return 'exhausted'


assert count_down(3) == 'exhausted'

# === raise in for-else caught outside ===
log = []
try:
    for i in range(2):
        log.append(i)
    else:
        raise ValueError('from else')
except ValueError as e:
    log.append(str(e))
assert log == [0, 1, 'from else']

# === try/finally inside a loop else clause ===
log = []
for i in range(2):
    pass
else:
    try:
        log.append('try')
    finally:
        log.append('finally')
assert log == ['try', 'finally']

# === break inside try/finally inside for-else targets the outer loop ===
log = []
for i in range(2):
    for j in range(1):
        log.append(f'inner {i} {j}')
    else:
        try:
            break
        finally:
            log.append('finally')
    log.append('unreached')
log.append('after')
assert log == ['inner 0 0', 'finally', 'after']

# === else after loop containing continue still runs ===
log = []
for i in range(3):
    if i == 1:
        continue
    log.append(i)
else:
    log.append('else')
assert log == [0, 2, 'else']
