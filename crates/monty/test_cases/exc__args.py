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
