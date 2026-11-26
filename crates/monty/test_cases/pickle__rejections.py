# /// monty
# xfail = ['cpython']
# ///

import pickle

for data in (
    b'',
    b'\x80\x05.',
    b'\x80\x05h\x00.',
    b'\x80\x05\x93.',
    b'\x80\x05]\x94',
    b'\x80\x05C\xff.',
    b'\x80\x05\x95\x01\x00\x00\x00\x00\x00\x00\x00K\x01.',
):
    try:
        pickle.loads(data)
    except ValueError:
        pass
    else:
        raise AssertionError('invalid or unsupported pickle was accepted')

nested = None
for _ in range(129):
    nested = [nested]
try:
    pickle.dumps(nested)
except RecursionError:
    pass
else:
    raise AssertionError('the writer exceeded its nesting limit')

try:
    pickle.dumps(complex(1, 2))
except TypeError:
    pass
else:
    raise AssertionError('the writer accepted an unsupported type')

assert pickle.loads(pickle.dumps({'still': 'usable'})) == {'still': 'usable'}
