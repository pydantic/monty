# Repeated search and excerpts, as in Flying Circus's subtitle/search workloads.
# Unicode before each match makes find/rfind convert byte offsets to character offsets.
block = 'Überprüfung 中文 🐍: ' * 32 + 'INC-XYZ-17\n'
text = block * 128
needle = 'INC-XYZ-17'
total = 0
matches = 0
position = 0

while True:
    position = text.find(needle, position)
    if position < 0:
        break
    excerpt = text[position - 3 : position + len(needle)]
    assert excerpt == '🐍: INC-XYZ-17'
    total += position
    matches += 1
    position += len(needle)

assert matches == 128
assert total == 4844864
assert text.rfind(needle) == 75125
assert text.count(needle) == 128
assert text.find(needle, len(text) - 10) == -1
