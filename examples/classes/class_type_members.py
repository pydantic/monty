"""ClassType members: class constants and classmethods, no instantiation.

On a `ClassType` the inherited policies apply to the class object itself:
`eager_attrs` sends class constants with the type and `allowed_methods`
exposes classmethods/staticmethods — useful for enum-style or factory
classes the sandbox should use but never construct.
"""

from pydantic_monty import ClassType, Monty


class Shape:
    SIDES = 4
    KIND = 'polygon'

    @classmethod
    def unit(cls) -> int:
        return cls.SIDES

    @staticmethod
    def double(n: int) -> int:
        return n * 2


with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassType(Shape, eager_attrs='all', allowed_methods={'unit', 'double'})
        result = session.feed_run(
            'assert Shape.KIND == "polygon"\nShape.unit() + Shape.double(10)',
            inputs={'Shape': wrapper},
        )

assert result == 24
print(f'class constants and classmethods work: {result}')
