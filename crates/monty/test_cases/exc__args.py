# === Exception .args attribute ===
try:
    raise ValueError('test message')
except ValueError as e:
    assert e.args == ('test message',)
    assert e.args[0] == 'test message'

try:
    raise ValueError()
except ValueError as e:
    assert e.args == ()

try:
    raise TypeError('type error')
except TypeError as e:
    assert e.args[0] == 'type error'

# === type() of an exception is the class its name evaluates to ===
try:
    raise ValueError('bad')
except ValueError as e:
    assert type(e) is ValueError
    assert type(e) == ValueError
    assert type(e) in (KeyError, ValueError)
    assert {ValueError: 'value'}[type(e)] == 'value'
    assert type(e)('again').args == ('again',)
    assert type(e).__name__ == 'ValueError'
assert type(KeyError('k')) is KeyError
assert type(KeyError('k')) is not LookupError
assert type(ZeroDivisionError()) != ArithmeticError
try:
    raise KeyError('first')
except KeyError as first:
    assert isinstance(KeyError('x'), type(first))
    try:
        raise KeyError('again')
    except type(first) as second:
        assert second.args == ('again',)
