# === Identity across nested call ===
err = ValueError('original')


class K:
    def __eq__(self, other):
        raise err


try:
    [K()].index(1)
    assert False, 'did not raise'
except ValueError as caught:
    assert caught is err
    assert caught.args == ('original',)

# === Chaining survives nested synchronous call ===
cause_err = KeyError('cause')
try:
    raise ValueError('nested') from cause_err
except ValueError as e:
    chained_err = e

assert chained_err.__cause__ is cause_err


class ChainedK:
    def __eq__(self, other):
        raise chained_err


try:
    [ChainedK()].index(1)
    assert False, 'did not raise'
except ValueError as caught:
    assert caught is chained_err
    assert caught.__cause__ is cause_err


# === hasattr and getattr with pre-built AttributeError in loop ===
class HasAttrTrap:
    def __init__(self):
        self.exc = AttributeError('trap')

    def __getattr__(self, name):
        raise self.exc


trap = HasAttrTrap()
for _ in range(50):
    assert not hasattr(trap, 'x')
    assert getattr(trap, 'x', 42) == 42


# === Hashing TypeError fallback ===
class UnhashableTrap:
    def __init__(self):
        self.exc = TypeError('cannot hash')

    def __hash__(self):
        raise self.exc


u = UnhashableTrap()
try:
    {u: 1}
    assert False, 'did not raise'
except TypeError as e:
    assert 'cannot hash' in str(e)

# === Sort key error identity ===
key_err = ValueError('sort key error')


def bad_key(x):
    raise key_err


try:
    sorted([1, 2, 3], key=bad_key)
    assert False, 'did not raise'
except ValueError as caught:
    assert caught is key_err
