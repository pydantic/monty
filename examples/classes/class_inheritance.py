"""Class inheritance inside Monty: single and multiple inheritance with C3 MRO.

Sandbox code can define class hierarchies with attribute resolution through MRO.
Instances returned to the host are read-only `MontyClassProxy` snapshots of the
instance state.
"""

from pydantic_monty import Monty, MontyClassProxy

CODE = """\
class Tagged:
    tag = 'v1'

class Base:
    greeting = 'Hello'

    def __init__(self, name: str):
        self.name = name

    def message(self) -> str:
        return f'{self.greeting}, {self.name}! [{self.tag}]'

class Sub(Base, Tagged):
    greeting = 'Welcome'

s = Sub('World')
assert s.message() == 'Welcome, World! [v1]'
assert isinstance(s, Sub)
assert isinstance(s, Base)
assert isinstance(s, Tagged)
s
"""

with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(CODE)

assert isinstance(result, MontyClassProxy)
assert result.name == 'Sub'
assert result.attributes == {'name': 'World'}
print(f'host received: {result!r}')
