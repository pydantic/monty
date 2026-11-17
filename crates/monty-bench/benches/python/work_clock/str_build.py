parts = []
for i in range(40_000):
    parts.append(f'{i:05d}-{i * 2!r}:' + str(i % 7))
text = '|'.join(parts)
assert len(text) == 554444
assert text.count('-') == 40_000
assert text.upper().lower() == text
