# mount-fs
# `csv` over `open()` files: the idiomatic shapes, including the ones that consume the
# file through a builtin rather than a `for` loop.
import csv

with open(root / 'data.csv', 'w', newline='') as f:
    f.write('name,qty\r\napple,3\r\n"pear, green",5\r\n')

# === Reading ===
with open(root / 'data.csv', newline='') as f:
    rows = [row for row in csv.reader(f)]
assert rows == [['name', 'qty'], ['apple', '3'], ['pear, green', '5']]
# Without newline='' the universal-newline translation leaves '\n', which the reader also accepts.
with open(root / 'data.csv') as f:
    assert list(csv.reader(f)) == [['name', 'qty'], ['apple', '3'], ['pear, green', '5']]
# `list()`, `next()` and `DictReader` all work on a file nobody has read yet.
with open(root / 'data.csv', newline='') as f:
    reader = csv.reader(f)
    assert next(reader) == ['name', 'qty']
    assert reader.line_num == 1
    assert list(reader) == [['apple', '3'], ['pear, green', '5']]
    assert reader.line_num == 3
with open(root / 'data.csv', newline='') as f:
    assert list(csv.DictReader(f)) == [{'name': 'apple', 'qty': '3'}, {'name': 'pear, green', 'qty': '5'}]
with open(root / 'data.csv', newline='') as f:
    assert [row['qty'] for row in csv.DictReader(f=f)] == ['3', '5']
with open(root / 'data.csv', newline='') as f:
    reader = csv.DictReader(f)
    assert reader.fieldnames == ['name', 'qty']
    assert [row['qty'] for row in reader] == ['3', '5']
    assert reader.line_num == 3
with open(root / 'data.csv', newline='') as f:
    assert sorted(len(row) for row in csv.reader(f)) == [2, 2, 2]
with open(root / 'data.csv', newline='') as f:
    header, *body = csv.reader(f)
    assert header == ['name', 'qty']
    assert len(body) == 2
# A file read in another way first still works.
with open(root / 'data.csv', newline='') as f:
    assert f.readline() == 'name,qty\r\n'
    assert list(csv.reader(f)) == [['apple', '3'], ['pear, green', '5']]
# Multi-line quoted fields span the file's lines.
with open(root / 'multi.csv', 'w', newline='') as f:
    f.write('a,"line one\nline two",c\n1,2,3\n')
with open(root / 'multi.csv', newline='') as f:
    assert list(csv.reader(f)) == [['a', 'line one\nline two', 'c'], ['1', '2', '3']]
# A binary file hands the reader bytes.
with open(root / 'data.csv', 'rb') as f:
    try:
        list(csv.reader(f))
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'iterator should return strings, not bytes (the file should be opened in text mode)'
# A missing file fails at open(), before the reader exists.
try:
    csv.reader(open(root / 'missing.csv'))
    assert False, 'expected FileNotFoundError'
except FileNotFoundError:
    pass

# === Writing ===
with open(root / 'out.csv', 'w', newline='') as f:
    writer = csv.writer(f)
    assert writer.writerow(['name', 'qty']) == 10
    assert writer.writerows([['apple', 3], ['pear, green', 5]]) is None
    assert writer.writerows([]) is None
assert (root / 'out.csv').read_bytes() == b'name,qty\r\napple,3\r\n"pear, green",5\r\n'
with open(root / 'out.csv', 'w', newline='') as f:
    writer = csv.DictWriter(f, ['name', 'qty'])
    assert writer.writeheader() == 10
    assert writer.writerow({'name': 'fig', 'qty': 1}) == 7
    assert writer.writerows([{'name': 'kiwi'}]) is None
assert (root / 'out.csv').read_bytes() == b'name,qty\r\nfig,1\r\nkiwi,\r\n'
# Round trip through the file.
with open(root / 'out.csv', newline='') as f:
    assert list(csv.DictReader(f)) == [{'name': 'fig', 'qty': '1'}, {'name': 'kiwi', 'qty': ''}]
# The unix dialect on a file opened with newline=''.
with open(root / 'unix.csv', 'w', newline='') as f:
    csv.writer(f, dialect='unix').writerow(['a', 1])
assert (root / 'unix.csv').read_bytes() == b'"a","1"\n'
# A writer over a read-only file fails at the first write, as the write() call would.
with open(root / 'data.csv') as f:
    writer = csv.writer(f)
    try:
        writer.writerow(['x'])
        assert False, 'expected UnsupportedOperation'
    except OSError as exc:
        assert str(exc) == 'not writable'
