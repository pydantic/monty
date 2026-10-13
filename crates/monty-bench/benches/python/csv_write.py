import csv

class Sink:
    def __init__(self):
        self.chars = 0

    def write(self, s):
        self.chars += len(s)

fieldnames = ['id', 'name', 'city', 'note', 'score']
rows = []
for i in range(2_000):
    rows.append([
        i,
        'name ' + str(i),
        'Springfield, IL' if i % 3 == 0 else 'Shelbyville',
        'said "hi"\nthen left' if i % 7 == 0 else 'ok',
        i * 0.25,
    ])
dict_rows = [dict(zip(fieldnames, row)) for row in rows]

sink = Sink()
w = csv.writer(sink)
for row in rows:
    w.writerow(row)
csv.writer(sink, quoting=csv.QUOTE_ALL).writerows(rows)
dw = csv.DictWriter(sink, fieldnames=fieldnames)
dw.writeheader()
dw.writerows(dict_rows)
sink.chars
