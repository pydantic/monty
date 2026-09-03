"""ClassInstance: expose a host object to the sandbox with an explicit policy.

`eager_attrs` sends attribute values with the object, `allowed_methods` lets the
sandbox call back into the real instance, and returning the object from sandbox
code hands the host back the ORIGINAL object, not a copy.
"""

from dataclasses import dataclass

from pydantic_monty import ClassInstance, Monty


@dataclass
class Person:
    name: str
    age: int

    def greeting(self) -> str:
        return f'hi {self.name}'


person = Person(name='Samuel', age=4)

with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(
            'assert user.name == "Samuel"\nassert user.greeting() == "hi Samuel"\nuser',
            inputs={'user': ClassInstance(person, eager_attrs='all', allowed_methods={'greeting'})},
        )

assert result is person  # identity round-trip
print(f'got back the original object: {result!r}')
