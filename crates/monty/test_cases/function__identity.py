def check_pair(first, second, first_result, second_result):
    alias = first
    assert first is not second
    assert id(first) != id(second)
    assert first != second
    assert first is alias
    assert first == alias
    assert hash(first) == hash(alias)
    assert first() == first_result
    assert second() == second_result
    assert len({first, second, alias}) == 2
    mapping = {first: 'first', second: 'second'}
    assert mapping[alias] == 'first'
    assert mapping[second] == 'second'
    assert second not in [first]
    return first, second, alias, mapping


def plain_factory():
    def inner():
        return 1

    return inner


examples = [check_pair(plain_factory(), plain_factory(), 1, 1)]


def default_factory(default):
    def inner(value=default):
        return value

    return inner


examples.append(check_pair(default_factory(1), default_factory(2), 1, 2))
assert examples[-1][0](3) == 3


def closure_factory():
    value = 1
    functions = []
    for _ in range(2):

        def inner():
            return value

        functions.append(inner)
    value = 2
    return functions


first, second = closure_factory()
examples.append(check_pair(first, second, 2, 2))


# Direct calls preserve argument ownership and callable aliases.
def call_factory():
    def choose(first, second, third):
        return first

    return choose


choose = call_factory()
payload = [1, 2, 3]
assert choose(payload, [], {}) is payload
assert choose(choose, [], {}) is choose
assert call_factory()(payload, [], {}) is payload


# Defaults, captured cells and explicit globals still use their own namespaces.
def with_default(first, second, third=payload):
    return third


assert with_default(None, None) is payload
assert with_default(None, None, choose) is choose
namespace = {'payload': payload}
exec('def pick(first, second, third):\n    return payload', namespace)
assert namespace['pick'](None, None, None) is payload


def captured_factory(item):
    def pick(first, second, third):
        return item

    return pick


assert captured_factory(payload)(None, None, None) is payload


import asyncio


async def async_choose(first, second, third):
    return first


assert asyncio.run(async_choose(payload, [], {})) is payload
assert asyncio.run(async_choose(async_choose, [], {})) is async_choose
