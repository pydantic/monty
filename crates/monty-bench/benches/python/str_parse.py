# Line-oriented text parsing: split, strip, lower, startswith, `in` and `int()` on real
# text, rather than the tiny strings `builtin_args` uses.

lines = []
for i in range(500):
    status = 'ACTIVE' if i % 3 else 'retired'
    lines.append('  Widget-' + str(i) + ' ;  ' + str((i * 13) % 400) + ' ; ' + status + ' ; north-' + str(i % 7) + '  ')
text = '\n'.join(lines)

count = 0
total = 0
for _ in range(10):
    for line in text.split('\n'):
        parts = line.split(';')
        name = parts[0].strip()
        qty = int(parts[1].strip())
        state = parts[2].strip().lower()
        zone = parts[3].strip()
        if state == 'active' and name.lower().startswith('widget') and 'north' in zone:
            count += 1
            total += qty
assert count + total == 655_040
