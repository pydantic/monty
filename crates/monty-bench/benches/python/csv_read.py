import csv

class Lines:
    def __init__(self):
        self.lines = []

    def write(self, s):
        self.lines.append(s)

fieldnames = ['id', 'name', 'city', 'note', 'score']
rows = []
for i in range(2_000):
    rows.append([
        float(i),
        'name ' + str(i),
        'Springfield, IL' if i % 3 == 0 else 'Shelbyville',
        'said "hi"\nthen left' if i % 7 == 0 else 'ok',
        i * 0.25,
    ])
sink = Lines()
csv.writer(sink, lineterminator='\n', quoting=csv.QUOTE_NONNUMERIC).writerows(rows)
lines = sink.lines

total = 0
for _ in range(5):
    for row in csv.reader(lines):
        total += len(row)
    for row in csv.reader(lines, quoting=csv.QUOTE_NONNUMERIC):
        total += int(row[4])
    for row in csv.DictReader(lines, fieldnames=fieldnames):
        total += len(row['name'])
total
