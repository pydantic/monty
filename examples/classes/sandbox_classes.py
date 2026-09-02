"""Classes defined inside Monty: instances reach the host as MontyClassProxy.

Sandbox code can define its own classes (including dataclasses). An instance
returned to the host is a read-only `MontyClassProxy` snapshot — `name`,
`is_dataclass`, `id`, and an `attributes` dict — never live code (see
`sandbox_round_trip.py` for passing it back).
"""

from pydantic_monty import Monty, MontyClassProxy

CODE = """\
from dataclasses import dataclass

@dataclass
class Point:
    x: int
    y: int

    def norm2(self) -> int:
        return self.x ** 2 + self.y ** 2

p = Point(3, 4)
assert p.norm2() == 25  # methods work inside the sandbox
p
"""

with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(CODE)

assert isinstance(result, MontyClassProxy)
assert result.name == 'Point'
assert result.is_dataclass is True
assert result.attributes == {'x': 3, 'y': 4}
print(f'host received: {result!r}')
