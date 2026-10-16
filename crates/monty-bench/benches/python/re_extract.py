# `finditer` with group capture and `re.split` over a large string, guarding the
# match-object and group-extraction wrapper around fancy-regex.

import re

parts = []
for i in range(300):
    parts.append('name=item' + str(i) + ' qty=' + str(i % 50) + ' price=' + str((i * 7) % 900))
text = ' '.join(parts)

pattern = re.compile(r'qty=(\d+) price=(\d+)')
total = 0
for _ in range(5):
    for m in pattern.finditer(text):
        total += int(m.group(1)) + int(m.group(2))
    words = re.split(r'\s+', text)
    total += len(words)
assert total == 652_500
