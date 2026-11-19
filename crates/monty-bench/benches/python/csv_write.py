# CSV formatting, how agents hand tabular results back: `writer.writerow` in a loop,
# `writerows` under `QUOTE_ALL`, and `DictWriter`. Fields mix plain values with ones that
# force quoting (commas, embedded quotes, newlines). The `write()` target is a class, since
# Monty has no `io.StringIO`; it counts characters written.

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
assert sink.chars == 265_163
