# === Empty container lengths ===
assert (len([]), len(()), len('')) == (0, 0, 0)

# === Large concatenations ===
lst = []
for i in range(100):
    lst += [i]
assert len(lst) == 100

s = ''
for i in range(100):
    s += 'x'
assert len(s) == 100

# === Self-concatenation ===
lst = [1]
lst += lst
lst += lst
assert lst == [1, 1, 1, 1]

# === Mod comparison in loop ===
count = 0
for i in range(100):
    if i % 7 == 0:
        count += 1
assert count == 15
