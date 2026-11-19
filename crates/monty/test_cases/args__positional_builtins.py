def check_type_error(call, message):
    try:
        call()
    except TypeError as exc:
        assert str(exc) == message
    else:
        assert False, message


assert len('é🙂') == 2
assert len(chr(0x1F642) + chr(0x301)) == 2
assert len(('x', 'y')) == 2
assert len([[], [], []]) == 3
assert len({'one': 1}.keys()) == 1
assert len({'a': 1, 'b': 2}) == 2
assert len(b'abc' * 4) == 12
assert len(range(3)) == 3
check_type_error(lambda: len(42), "object of type 'int' has no len()")

assert divmod(-7, 3) == (-3, 2)
check_type_error(lambda: divmod(1), 'divmod expected 2 arguments, got 1')
check_type_error(lambda: divmod(1, 2, 3), 'divmod expected 2 arguments, got 3')
check_type_error(lambda: divmod(1, 2, x=[]), 'divmod() takes no keyword arguments')

items = [10, 20]
assert items.pop() == 20
assert items.pop(0) == 10
check_type_error(lambda: items.pop(None), "'NoneType' object cannot be interpreted as an integer")
check_type_error(lambda: items.pop(1, 2), 'pop expected at most 1 argument, got 2')
check_type_error(lambda: items.pop(index=0), 'list.pop() takes no keyword arguments')

fallback = []
mapping = {'hit': fallback}
assert mapping.get('miss') is None
assert mapping.get('miss', None) is None
assert mapping.get('miss', fallback) is fallback
assert mapping.get('hit', []) is fallback
check_type_error(lambda: mapping.get(), 'get expected at least 1 argument, got 0')
check_type_error(lambda: mapping.get(1, 2, 3), 'get expected at most 2 arguments, got 3')
check_type_error(lambda: mapping.get(1, 2, 3, key=[]), 'dict.get() takes no keyword arguments')
check_type_error(lambda: mapping.get([], []), "cannot use 'list' as a dict key (unhashable type: 'list')")

views = {}
values = views.values()
items_view = views.items()
assert list(values) == []
assert list(items_view) == []
views['a'] = 1
views['b'] = 2
views['a'] = 3
assert views.pop('b') == 2
assert list(values) == [3]
assert list(items_view) == [('a', 3)]
for positional in [(), ([],)]:
    check_type_error(lambda: views.values(*positional, wrong=[]), 'dict.values() takes no keyword arguments')
    check_type_error(lambda: views.items(*positional, wrong=[]), 'dict.items() takes no keyword arguments')

fallback = []
target = {'key': fallback}
assert target.pop('key', []) is fallback
assert target == {}
assert target.pop([], fallback) is fallback
assert target.pop('missing', None) is None
try:
    target.pop('missing')
except KeyError as exc:
    assert exc.args == ('missing',)
else:
    assert False, 'pop without a default must raise KeyError'
target['key'] = 1
check_type_error(lambda: target.pop([], fallback), "cannot use 'list' as a dict key (unhashable type: 'list')")
check_type_error(lambda: target.pop(), 'pop expected at least 1 argument, got 0')
check_type_error(lambda: target.pop(1, 2, 3), 'pop expected at most 2 arguments, got 3')
check_type_error(lambda: target.pop(1, 2, 3, key=[]), 'dict.pop() takes no keyword arguments')
assert target == {'key': 1}

defaults = {}
check_type_error(lambda: defaults.setdefault(), 'setdefault expected at least 1 argument, got 0')
check_type_error(lambda: defaults.setdefault(1, 2, 3), 'setdefault expected at most 2 arguments, got 3')
check_type_error(lambda: defaults.setdefault(key=1), 'dict.setdefault() takes no keyword arguments')
check_type_error(lambda: defaults.setdefault(1, 2, 3, default=[]), 'dict.setdefault() takes no keyword arguments')
assert defaults == {}
assert defaults.setdefault('omitted') is None
assert defaults.setdefault('explicit', None) is None
assert defaults == {'omitted': None, 'explicit': None}
