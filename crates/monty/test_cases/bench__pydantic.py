# /// monty
# cpython-site-packages = true
# ///
# Every `bench__*.py` test case is also a `monty-bench` benchmark (benches/main.rs):
# validation and serialisation of a nested dataclass, in Python and JSON mode.
from dataclasses import dataclass, field
from datetime import datetime

from pydantic import TypeAdapter, ValidationError


@dataclass
class Item:
    sku: str
    quantity: int
    price: float
    tags: list[str] = field(default_factory=list)


@dataclass
class Order:
    id: int
    customer: str
    placed: datetime
    items: list[Item]
    notes: str | None = None
    totals: dict[str, float] = field(default_factory=dict)


OrderAdapter = TypeAdapter(list[Order])

orders = []
for n in range(20):
    orders.append(
        {
            'id': n,
            'customer': f'customer-{n}',
            'placed': '2024-05-06T07:08:09Z',
            'items': [
                {'sku': f'sku-{n}-{i}', 'quantity': str(i + 1), 'price': i * 1.5, 'tags': ['a', 'b']} for i in range(5)
            ],
            'notes': None if n % 2 else 'rush',
            'totals': {'net': 10.0 * n, 'tax': n * 2.0},
        }
    )

validated = OrderAdapter.validate_python(orders)
assert len(validated) == 20
assert validated[3].items[2].quantity == 3
assert validated[3].placed.year == 2024

document = OrderAdapter.dump_json(validated)
assert document.startswith(b'[{"id":0,"customer":"customer-0","placed":"2024-05-06T07:08:09Z"')
from_json = OrderAdapter.validate_json(document)
assert from_json == validated

as_python = OrderAdapter.dump_python(validated, mode='json', exclude_none=True)
assert as_python[0]['items'][1] == {'sku': 'sku-0-1', 'quantity': 2, 'price': 1.5, 'tags': ['a', 'b']}
assert 'notes' not in as_python[1]

bad = [dict(order, items=[dict(item, quantity='many') for item in order['items']]) for order in orders[:5]]
try:
    OrderAdapter.validate_python(bad)
    assert False, 'expected a ValidationError'
except ValidationError as exc:
    assert exc.error_count() == 25
