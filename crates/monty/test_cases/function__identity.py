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
