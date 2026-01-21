# === Exception .args attribute ===
try:
    raise ValueError('test message')
except ValueError as e:
    assert e.args == ('test message',), 'args is tuple with message'
    assert e.args[0] == 'test message', 'args[0] is the message'

try:
    raise ValueError()
except ValueError as e:
    assert e.args == (), 'no-arg exception has empty args'

try:
    raise TypeError('type error')
except TypeError as e:
    assert e.args[0] == 'type error', 'works for other exception types'
