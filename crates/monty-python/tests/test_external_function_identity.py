"""Identity and equality semantics for external function inputs (#347, #345).

Monty represents external function inputs in two ways depending on whether the
function's `__name__` was interned during parsing:

- inline `Value::ExtFunction(StringId)` when the name appears in source
- heap `HeapData::ExtFunction(String)` otherwise

Both representations refer to the same logical callable and must therefore be
`is`-, `==`-, `id()`-, and `hash()`-identical based on the name string. The same
callable passed twice as input must satisfy these invariants regardless of
which path the conversion takes.
"""

from inline_snapshot import snapshot

import pydantic_monty


def foo():
    pass


def bar():
    pass


def test_same_callable_identical_when_name_not_in_source():
    m = pydantic_monty.Monty('(a is b, a == b)', inputs=['a', 'b'])
    assert m.run(inputs={'a': foo, 'b': foo}) == snapshot((True, True))


def test_same_callable_identical_when_name_in_source():
    # `foo = None` interns the string "foo" during parsing, so the input
    # conversion takes the inline `Value::ExtFunction(StringId)` path.
    m = pydantic_monty.Monty('foo = None\n(a is b, a == b)', inputs=['a', 'b'])
    assert m.run(inputs={'a': foo, 'b': foo}) == snapshot((True, True))


def test_id_matches_is_for_same_callable():
    m = pydantic_monty.Monty('id(a) == id(b)', inputs=['a', 'b'])
    assert m.run(inputs={'a': foo, 'b': foo}) == snapshot(True)


def test_hash_matches_equality_for_same_callable():
    m = pydantic_monty.Monty('hash(a) == hash(b)', inputs=['a', 'b'])
    assert m.run(inputs={'a': foo, 'b': foo}) == snapshot(True)


def test_callable_as_dict_key_round_trips():
    # Inserting under one binding and reading under the other relies on
    # consistent hash + equality across the inputs.
    m = pydantic_monty.Monty('d = {a: 42}\nd[b]', inputs=['a', 'b'])
    assert m.run(inputs={'a': foo, 'b': foo}) == snapshot(42)


def test_distinct_named_callables_remain_distinct():
    # Different __name__ values must not collapse: the most we collapse on is
    # the function name, and these have different names.
    m = pydantic_monty.Monty('(a is b, a == b)', inputs=['a', 'b'])
    assert m.run(inputs={'a': foo, 'b': bar}) == snapshot((False, False))


def test_inline_callable_exports_as_function_object():
    # Round-trip through the inline path (the bug from #345): when the
    # function name is interned in source, the inline `Value::ExtFunction`
    # used to fall through to `repr_or_error` and export as the repr string
    # `<function 'foo' external>` rather than the name `foo`.
    m = pydantic_monty.Monty('foo = None\nx', inputs=['x'])
    assert m.run(inputs={'x': foo}) == snapshot('foo')


def test_callable_export_stable_across_source_mention():
    # Same callable, two source variants. The exported value must be the same
    # representation regardless of whether the name was interned.
    m1 = pydantic_monty.Monty('x', inputs=['x'])
    m2 = pydantic_monty.Monty('foo = None\nx', inputs=['x'])
    assert m1.run(inputs={'x': foo}) == m2.run(inputs={'x': foo})
