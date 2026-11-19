def check_type_error(call, message):
    try:
        call()
    except TypeError as exc:
        assert str(exc) == message, str(exc)
    else:
        assert False, message


assert len('é🙂') == 2
assert len(chr(0x1F642) + chr(0x301)) == 2
assert len(('x', 'y')) == 2
assert len([[], [], []]) == 3
assert len({'one': 1}.keys()) == 1
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
