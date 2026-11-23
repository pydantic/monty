# Tests cleanup when keyword-only parsers reject calls before consuming kwargs.
#
# Shared kwarg parsing helpers are safe only if callers guard owned kwargs before
# any early arity/type errors. These handled exceptions should not leak the
# heap-backed kwarg values.


def _error_message(*, monty, cpython):
    import sys

    return monty if sys.platform == 'monty' else cpython


sorted_key = ['sorted-key']
sort_key = ['list-sort-key']
items = [3, 2, 1]

try:
    sorted(key=sorted_key)
    assert False, 'sorted() with no positional args should raise TypeError'
except TypeError as e:
    assert e.args == _error_message(
        monty=('sorted() takes exactly 1 argument (0 given)',),
        cpython=('sorted expected 1 argument, got 0',),
    )

try:
    items.sort(1, key=sort_key)
    assert False, 'list.sort() should reject positional args before consuming kwargs'
except TypeError as exc:
    assert str(exc) == _error_message(
        monty='sort() takes at most 0 positional arguments (1 given)',
        cpython='sort() takes no positional arguments',
    )

# The handled exceptions above must not retain references to the kwarg payloads.
# ref-counts={'sorted_key': 1, 'sort_key': 1, 'items': 1}
