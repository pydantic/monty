"""Object identity across the boundary.

Values cross as one node arena per message, so a sub-object the sandbox
references twice arrives as one host object, a host object passed twice is one
sandbox object, and a cycle arrives as its placeholder string.
"""

from __future__ import annotations

from collections.abc import Iterable
from typing import Any, cast

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import ClassInstance, MontyClassProxy, MontySession


def distinct_containers(value: object) -> int:
    """Counts the distinct list, tuple, dict and set objects reachable from `value`."""
    seen: set[int] = set()
    stack: list[object] = [value]
    while stack:
        item = stack.pop()
        key = id(item)
        if key in seen:
            continue
        if isinstance(item, dict):
            mapping = cast('dict[object, object]', item)
            stack.extend(mapping.keys())
            stack.extend(mapping.values())
        elif isinstance(item, (list, tuple, set, frozenset)):
            stack.extend(cast('Iterable[object]', item))
        else:
            continue
        seen.add(key)
    return len(seen)


# === sandbox → host ===


def test_shared_child_is_one_host_object(monty_run: RunMonty):
    result = monty_run('x = [1]\n[x, x]')
    assert result == snapshot([[1], [1]])
    assert result[0] is result[1]


def test_dict_key_shared_with_value(monty_run: RunMonty):
    result = monty_run('k = (1, 2)\n{k: k}')
    assert result == snapshot({(1, 2): (1, 2)})
    ((key, value),) = result.items()
    assert key is value


def test_shared_graph_is_linear_in_sandbox_objects(monty_run: RunMonty):
    # 36 lists that a tree export would expand into 753,663 nodes
    code = """
x = [0]
for _ in range(20):
    x = [x]
for _ in range(15):
    x = [x, x]
x
"""
    result = monty_run(code)
    assert distinct_containers(result) == snapshot(36)
    assert result[0] is result[1]


def test_many_references_to_one_object(monty_run: RunMonty):
    # a reference costs one arena id, so 200,000 references to one list cross
    # as three nodes and decode to one host object referenced 200,000 times
    result = monty_run('x = [1]\n[x] * 200_000')
    assert len(result) == 200_000
    assert all(item is result[0] for item in result)


def test_many_cycles_are_one_placeholder_each(monty_run: RunMonty):
    result = monty_run('xs = [[] for _ in range(10_000)]\nfor x in xs:\n    x.append(x)\nxs')
    assert len(result) == 10_000
    assert all(x == ['[...]'] for x in result)


@pytest.mark.parametrize(
    'code, expected',
    [
        ('x = []\nx.append(x)\nx', ['[...]']),
        ("d = {}\nd['self'] = d\nd", {'self': '{...}'}),
        ('x = []\nt = (x,)\nx.append(t)\nx', [('[...]',)]),
    ],
)
def test_cycle_arrives_as_its_placeholder(monty_run: RunMonty, code: str, expected: object):
    assert monty_run(code) == expected


def test_shared_sandbox_instance_is_one_proxy(monty_run: RunMonty):
    result = monty_run('class Foo:\n    pass\nfoo = Foo()\n[foo, foo]')
    assert isinstance(result[0], MontyClassProxy)
    assert result[0] is result[1]


def test_instance_cycle_through_attrs(monty_run: RunMonty):
    result = monty_run('class Foo:\n    pass\nfoo = Foo()\nfoo.me = foo\nfoo')
    assert isinstance(result, MontyClassProxy)
    assert result.attributes == snapshot({'me': '...'})


# === host → sandbox ===


def test_shared_child_is_one_sandbox_object(monty_run: RunMonty):
    y = [1]
    assert monty_run('xs[0] is xs[1]', inputs={'xs': [y, y]}) is True


def test_object_shared_across_inputs(monty_run: RunMonty):
    y = [1]
    assert monty_run('a is b', inputs={'a': y, 'b': y}) is True


def test_wrapper_shared_across_inputs(monty_run: RunMonty):
    class Foo:
        pass

    foo = Foo()
    wrapper = ClassInstance(foo)
    result = monty_run('[a is b, a]', inputs={'a': wrapper, 'b': wrapper})
    assert result[0] is True
    assert result[1] is foo


def test_cyclic_host_return_value_raises_in_sandbox(monty_run: RunMonty):
    def f() -> list[Any]:
        x: list[Any] = []
        x.append(x)
        return x

    code = """
try:
    f()
    result = 'no error'
except ValueError as e:
    result = str(e)
result
"""
    assert monty_run(code, external_lookup={'f': f}) == snapshot('Circular reference detected')


# === host functions ===


def test_argument_passed_twice_is_one_host_object(session: MontySession):
    seen: list[tuple[object, object]] = []

    def f(a: object, b: object) -> None:
        seen.append((a, b))

    session.feed_run('x = [1]\nf(x, x)', external_lookup={'f': f})
    assert seen == snapshot([([1], [1])])
    assert seen[0][0] is seen[0][1]


def test_separate_calls_get_separate_objects(session: MontySession):
    seen: list[object] = []

    def f(a: object) -> None:
        seen.append(a)

    session.feed_run('x = [1]\nf(x)\nf(x)', external_lookup={'f': f})
    assert seen == snapshot([[1], [1]])
    assert seen[0] is not seen[1]
