# Fresh heap strings exercise initial character counting and repeated length queries.
body = 'aé中🐍' * 4096
texts = [body + str(i) for i in range(128)]
total = 0

for text in texts:
    total += len(text)

assert total == 2097426

total = 0

for _ in range(64):
    for text in texts:
        total += len(text)

assert total == 134235264
