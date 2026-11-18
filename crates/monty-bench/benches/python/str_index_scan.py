# Index-based scanning of a long ASCII string, the way agent code pulls JSON objects
# out of model output by brace matching: `while i < len(text)` with `text[i]`, plus
# `find(sub, start)` to skip ahead. Guards O(1) `len`/indexing/`find` offsets on ASCII `str`.

chunks = []
for i in range(400):
    chunks.append('Step ' + str(i) + ': calling tool.\n```json\n{"id": ' + str(i) + ', "args": {"q": "item-' + str(i) + '", "n": ' + str(i % 9) + '}}\n```\n')
text = ''.join(chunks)

objects = []
i = 0
while i < len(text):
    if text[i] == '{':
        depth = 0
        j = i
        while j < len(text):
            c = text[j]
            if c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
                if depth == 0:
                    break
            j += 1
        objects.append(text[i : j + 1])
        i = j + 1
    else:
        i += 1

fences = 0
pos = text.find('```json')
while pos != -1:
    fences += 1
    pos = text.find('```json', pos + 1)

assert len(objects) == 400 and fences == 400, (len(objects), fences)
assert len(text) == 32_870
