# `TypeAdapter.dump_python` and `dump_json` on the 20-order nested dataclass payload of
# `pydantic_validate_python.py`, validated once and serialised 20 times per run.
from dataclasses import dataclass, field
from datetime import datetime

from pydantic import TypeAdapter


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


adapter = TypeAdapter(list[Order])

orders = [
    {
        'id': n,
        'customer': f'customer-{n}',
        'placed': '2024-05-06T07:08:09Z',
        'items': [{'sku': f'sku-{n}-{i}', 'quantity': i + 1, 'price': i * 1.5, 'tags': ['a', 'b']} for i in range(5)],
        'notes': None if n % 2 else 'rush',
        'totals': {'net': 10.0 * n, 'tax': n * 2.0},
    }
    for n in range(20)
]
validated = adapter.validate_python(orders)

size = 0
for _ in range(20):
    size += len(adapter.dump_python(validated, mode='json', exclude_none=True))
    size += len(adapter.dump_json(validated))
assert size == 170700
