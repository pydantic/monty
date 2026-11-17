# Straight-line code as a script or a test writes it: small values built once, formatted,
# sliced, searched and checked, with little arithmetic and no hot inner loop.
class Order:
    def __init__(self, ident, items, customer):
        self.ident = ident
        self.items = items
        self.customer = customer

    def total(self):
        return sum(price * count for _, price, count in self.items)

    def label(self):
        return f'{self.customer.title()} #{self.ident:04d}'


def summarize(order):
    names = [name for name, _, _ in order.items]
    header = f'{order.label()}: {len(names)} items'
    lines = [header]
    for name, price, count in order.items:
        lines.append(f'  {name.ljust(8)} {count} x {price:.2f}')
    lines.append(f'  total {order.total():.2f}')
    return '\n'.join(lines)


checked = 0
for ident in range(4_000):
    items = [('apple', 0.5, 3), ('pear', 0.75, 2), ('melon', 2.25, 1)]
    order = Order(ident, items, 'alice smith')
    text = summarize(order)
    parts = text.split('\n')
    assert len(parts) == 5
    assert parts[0] == f'Alice Smith #{ident:04d}: 3 items'
    assert parts[-1].strip() == 'total 5.25'
    assert parts[1:4][0].startswith('  apple')
    assert 'pear' in text and 'plum' not in text
    assert text.count(' x ') == 3
    first, *rest = parts
    assert rest[-1] == parts[-1]
    record = {'id': ident, 'customer': order.customer, 'names': [name for name, _, _ in items]}
    assert record['names'][::-1] == ['melon', 'pear', 'apple']
    assert sorted(record) == ['customer', 'id', 'names']
    assert isinstance(record['id'], int) and record['id'] >= 0
    key = (order.customer.upper(), ident % 7)
    assert key[0].replace(' ', '_') == 'ALICE_SMITH'
    assert tuple(reversed(key))[1] == 'ALICE SMITH'
    assert str(order.total()) == '5.25' and repr(key[1]) == str(ident % 7)
    checked += 1
assert checked == 4_000
