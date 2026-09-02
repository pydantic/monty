"""ClassType: let sandbox code instantiate a host class.

`init=True` grants construction — the call runs host-side and the new
instance crosses back governed by the `instance_*` policies. Without
`init=True` the sandbox gets `TypeError: cannot instantiate host class ...`.
"""

from dataclasses import dataclass

from pydantic_monty import ClassType, Monty, MontyRuntimeError


@dataclass
class Person:
    name: str
    age: int

    def greeting(self) -> str:
        return f'hi {self.name}'


with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassType(Person, init=True, instance_eager_attrs='all', instance_allowed_methods='all')
        result = session.feed_run('p = Person("Samuel", 4)\np.greeting()', inputs={'Person': wrapper})
        assert result == 'hi Samuel'
        print(f'constructed in the sandbox: {result!r}')

    with pool.checkout() as session:
        try:
            session.feed_run('Person("Samuel", 4)', inputs={'Person': ClassType(Person)})  # init defaults to False
        except MontyRuntimeError as exc:
            print(f'construction denied: {exc}')
        else:
            raise AssertionError('expected construction to be denied')
