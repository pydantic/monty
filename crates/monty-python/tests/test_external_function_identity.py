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

from conftest import RunMonty
from inline_snapshot import snapshot


def foo():
    pass


def bar():
    pass


def test_same_callable_identical_when_name_not_in_source(monty_run: RunMonty):
    assert monty_run('(a is b, a == b)', inputs={'a': foo, 'b': foo}) == snapshot((True, True))


def test_same_callable_identical_when_name_in_source(monty_run: RunMonty):
    # `foo = None` interns the string "foo" during parsing, so the input
    # conversion takes the inline `Value::ExtFunction(StringId)` path.
    assert monty_run('foo = None\n(a is b, a == b)', inputs={'a': foo, 'b': foo}) == snapshot((True, True))


def test_id_matches_is_for_same_callable(monty_run: RunMonty):
    assert monty_run('id(a) == id(b)', inputs={'a': foo, 'b': foo}) == snapshot(True)


def test_hash_matches_equality_for_same_callable(monty_run: RunMonty):
    assert monty_run('hash(a) == hash(b)', inputs={'a': foo, 'b': foo}) == snapshot(True)


def test_callable_as_dict_key_round_trips(monty_run: RunMonty):
    # Inserting under one binding and reading under the other relies on
    # consistent hash + equality across the inputs.
    assert monty_run('d = {a: 42}\nd[b]', inputs={'a': foo, 'b': foo}) == snapshot(42)


def test_distinct_named_callables_remain_distinct(monty_run: RunMonty):
    # Different __name__ values must not collapse: the most we collapse on is
    # the function name, and these have different names.
    assert monty_run('(a is b, a == b)', inputs={'a': foo, 'b': bar}) == snapshot((False, False))


def test_inline_callable_exports_as_function_object(monty_run: RunMonty):
    # Round-trip through the inline path (the bug from #345): when the
    # function name is interned in source, the inline `Value::ExtFunction`
    # used to fall through to `repr_or_error` and export as the repr string
    # `<function 'foo' external>` rather than the name `foo`.
    assert monty_run('foo = None\nx', inputs={'x': foo}) == snapshot('foo')


def test_callable_export_stable_across_source_mention(monty_run: RunMonty):
    # Same callable, two source variants. The exported value must be the same
    # representation regardless of whether the name was interned.
    assert monty_run('x', inputs={'x': foo}) == monty_run('foo = None\nx', inputs={'x': foo})
