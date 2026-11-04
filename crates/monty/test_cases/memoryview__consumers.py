import base64
import binascii
import copy
import hashlib

view = memoryview(b'password')
salt = memoryview(b'salt')
assert hashlib.blake2b(b'data', key=view).digest() == hashlib.blake2b(b'data', key=b'password').digest()
assert hashlib.pbkdf2_hmac('sha256', view, salt, 2) == hashlib.pbkdf2_hmac('sha256', b'password', b'salt', 2)
for operation in [0, 1, 2]:
    strided = memoryview(b'abcd')[::2]
    try:
        if operation == 0:
            hashlib.blake2b(b'data', key=strided)
        elif operation == 1:
            hashlib.pbkdf2_hmac('sha256', strided, b'salt', 2)
        else:
            hashlib.pbkdf2_hmac('sha256', b'password', strided, 2)
    except BufferError:
        pass
    else:
        assert False, operation

for value in [view, iter(view)]:
    for copier in [copy.copy, copy.deepcopy]:
        try:
            copier(value)
        except TypeError as exc:
            expected = (
                'cannot pickle memoryview objects'
                if type(value) is memoryview
                else "cannot pickle 'memory_iterator' object"
            )
            assert str(exc) == expected
        else:
            assert False

for code in ['e', 'f', 'd', 'c', '?', 'Q']:
    raw = bytearray(8)
    typed = memoryview(raw).cast(code)
    assert hashlib.sha256(typed).digest() == hashlib.sha256(b'\0' * 8).digest()
    assert hashlib.blake2b(key=typed).digest() == hashlib.blake2b(key=b'\0' * 8).digest()
    assert base64.b64encode(typed) == base64.b64encode(b'\0' * 8)
    assert binascii.hexlify(typed) == b'00' * 8
    assert binascii.crc32(typed) == binascii.crc32(b'\0' * 8)
    assert typed.toreadonly().tolist() == typed.tolist()
    assert all([type(item) is type(typed[0]) for item in typed.tolist()])
