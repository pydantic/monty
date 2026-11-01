# /// monty
# [bench]
# data = 'data/subtitles_en.txt'
# ///

# Locate mentions in original subtitle-style dialogue for an agent's excerpts.
# Excerpts add character-index conversion and slicing around each match.
text = DATA
needle = 'Alex Rivera'
position = 0
count = 0
checksum = 0
while True:
    position = text.find(needle, position)
    if position < 0:
        break
    count += 1
    checksum += position + len(text[position : position + 48])
    position += len(needle)

assert count == 80
assert checksum == 1685732
assert text.count(needle) == 80
assert text.rfind(needle) == 41813
