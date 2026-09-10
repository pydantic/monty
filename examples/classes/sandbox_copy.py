"""Sandbox mutations never touch the host object.

In-sandbox `setattr` on a host instance succeeds — on the sandbox copy only.
The wrapped host object is unchanged, and host instances are unhashable in
the sandbox (they define equality by attrs), like an `eq` dataclass.
"""

from dataclasses import dataclass

from pydantic_monty import ClassInstance, Monty


@dataclass(frozen=True)
class Point:
    x: int
    y: int


point = Point(1, 2)

with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(
            'p.x = 99\np.x',
            inputs={'p': ClassInstance(point, eager_attrs='all')},
        )

assert result == 99
assert point.x == 1  # the host object is untouched
print(f'sandbox copy saw x={result}, host object still {point}')
