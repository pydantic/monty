import pytest
from inline_snapshot import snapshot

import monty


def test_none_input():
    m = monty.Monty('x is None', inputs=['x'])
    assert m.run(inputs={'x': None}) is True


def test_none_output():
    m = monty.Monty('None')
    assert m.run() is None


def test_bool_true():
    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': True})
    assert result is True
    assert type(result) is bool


def test_bool_false():
    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': False})
    assert result is False
    assert type(result) is bool


def test_int():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 42}) == snapshot(42)
    assert m.run(inputs={'x': -100}) == snapshot(-100)
    assert m.run(inputs={'x': 0}) == snapshot(0)


def test_float():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 3.14}) == snapshot(3.14)
    assert m.run(inputs={'x': -2.5}) == snapshot(-2.5)
    assert m.run(inputs={'x': 0.0}) == snapshot(0.0)


def test_string():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 'hello'}) == snapshot('hello')
    assert m.run(inputs={'x': ''}) == snapshot('')
    assert m.run(inputs={'x': 'unicode: éè'}) == snapshot('unicode: éè')


def test_bytes():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': b'hello'}) == snapshot(b'hello')
    assert m.run(inputs={'x': b''}) == snapshot(b'')
    assert m.run(inputs={'x': b'\x00\x01\x02'}) == snapshot(b'\x00\x01\x02')


def test_list():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': [1, 2, 3]}) == snapshot([1, 2, 3])
    assert m.run(inputs={'x': []}) == snapshot([])
    assert m.run(inputs={'x': ['a', 'b']}) == snapshot(['a', 'b'])


def test_tuple():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': (1, 2, 3)}) == snapshot((1, 2, 3))
    assert m.run(inputs={'x': ()}) == snapshot(())
    assert m.run(inputs={'x': ('a',)}) == snapshot(('a',))


def test_dict():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': {'a': 1, 'b': 2}}) == snapshot({'a': 1, 'b': 2})
    assert m.run(inputs={'x': {}}) == snapshot({})


def test_set():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': {1, 2, 3}}) == snapshot({1, 2, 3})
    assert m.run(inputs={'x': set()}) == snapshot(set())


def test_frozenset():
    m = monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': frozenset([1, 2, 3])}) == snapshot(frozenset({1, 2, 3}))
    assert m.run(inputs={'x': frozenset()}) == snapshot(frozenset())


def test_ellipsis_input():
    m = monty.Monty('x is ...', inputs=['x'])
    assert m.run(inputs={'x': ...}) is True


def test_ellipsis_output():
    m = monty.Monty('...')
    assert m.run() is ...


def test_nested_list():
    m = monty.Monty('x', inputs=['x'])
    nested = [[1, 2], [3, [4, 5]]]
    assert m.run(inputs={'x': nested}) == snapshot([[1, 2], [3, [4, 5]]])


def test_nested_dict():
    m = monty.Monty('x', inputs=['x'])
    nested = {'a': {'b': {'c': 1}}}
    assert m.run(inputs={'x': nested}) == snapshot({'a': {'b': {'c': 1}}})


def test_mixed_nested():
    m = monty.Monty('x', inputs=['x'])
    mixed = {'list': [1, 2], 'tuple': (3, 4), 'nested': {'set': {5, 6}}}
    result = m.run(inputs={'x': mixed})
    assert result['list'] == snapshot([1, 2])
    assert result['tuple'] == snapshot((3, 4))
    assert result['nested']['set'] == snapshot({5, 6})


def test_list_output():
    m = monty.Monty('[1, 2, 3]')
    assert m.run() == snapshot([1, 2, 3])


def test_dict_output():
    m = monty.Monty("{'a': 1, 'b': 2}")
    assert m.run() == snapshot({'a': 1, 'b': 2})


def test_tuple_output():
    m = monty.Monty('(1, 2, 3)')
    assert m.run() == snapshot((1, 2, 3))


def test_set_output():
    m = monty.Monty('{1, 2, 3}')
    assert m.run() == snapshot({1, 2, 3})


# === Exception types ===


def test_exception_input():
    m = monty.Monty('x', inputs=['x'])
    exc = ValueError('test error')
    result = m.run(inputs={'x': exc})
    assert isinstance(result, ValueError)
    assert str(result) == snapshot('test error')


def test_exception_output():
    m = monty.Monty('ValueError("created")')
    result = m.run()
    assert isinstance(result, ValueError)
    assert str(result) == snapshot('created')


@pytest.mark.parametrize('exc_class', [ValueError, TypeError, RuntimeError, AttributeError], ids=repr)
def test_exception_roundtrip(exc_class: type[Exception]):
    m = monty.Monty('x', inputs=['x'])
    exc = exc_class('message')
    result = m.run(inputs={'x': exc})
    assert type(result) is exc_class
    assert str(result) == snapshot('message')


def test_exception_subclass_input():
    """Custom exception subtypes are converted to their nearest supported base."""

    class MyError(ValueError):
        pass

    m = monty.Monty('x', inputs=['x'])
    exc = MyError('custom')
    result = m.run(inputs={'x': exc})
    # Custom exception becomes ValueError (nearest supported type)
    assert type(result) is ValueError
    assert str(result) == snapshot('custom')


# === Subtype coercion ===
# Monty converts Python subclasses to their base types since it doesn't
# have Python's class system.


def test_int_subclass_input():
    class MyInt(int):
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyInt(42)})
    assert type(result) is int
    assert result == snapshot(42)


def test_str_subclass_input():
    class MyStr(str):
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyStr('hello')})
    assert type(result) is str
    assert result == snapshot('hello')


def test_list_subclass_input():
    class MyList(list[int]):
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyList([1, 2, 3])})
    assert type(result) is list
    assert result == snapshot([1, 2, 3])


def test_dict_subclass_input():
    class MyDict(dict[str, int]):
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyDict({'a': 1})})
    assert type(result) is dict
    assert result == snapshot({'a': 1})


def test_tuple_subclass_input():
    class MyTuple(tuple[int, ...]):
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MyTuple((1, 2))})
    assert type(result) is tuple
    assert result == snapshot((1, 2))


def test_set_subclass_input():
    class MySet(set[int]):
        pass

    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': MySet({1, 2})})
    assert type(result) is set
    assert result == snapshot({1, 2})


def test_bool_preserves_type():
    """Bool is a subclass of int but should be preserved as bool."""
    m = monty.Monty('x', inputs=['x'])
    result = m.run(inputs={'x': True})
    assert type(result) is bool
    assert result is True


def test_return_int():
    m = monty.Monty('x = 4\ntype(x)')
    result = m.run()
    assert result is int

    m = monty.Monty('int')
    result = m.run()
    assert result is int


def test_return_exception():
    m = monty.Monty('x = ValueError()\ntype(x)')
    result = m.run()
    assert result is ValueError

    m = monty.Monty('ValueError')
    result = m.run()
    assert result is ValueError


def test_return_builtin():
    m = monty.Monty('len')
    result = m.run()
    assert result is len
