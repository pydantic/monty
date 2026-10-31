import math

# CPython's positional-count errors name the bare function, never its type or module:
# `'abc'.find()` raises `find expected ...`, not `str.find expected ...`.
for compute, message in [
    (lambda: 'abc'.find(), 'find expected at least 1 argument, got 0'),
    (lambda: 'abc'.count('a', 1, 2, 3), 'count expected at most 3 arguments, got 4'),
    (lambda: 'abc'.startswith(), 'startswith expected at least 1 argument, got 0'),
    (lambda: 'abc'.center(), 'center expected at least 1 argument, got 0'),
    (lambda: 'abc'.strip(' ', ' '), 'strip expected at most 1 argument, got 2'),
    (lambda: b'abc'.rfind(), 'rfind expected at least 1 argument, got 0'),
    (lambda: b'abc'.endswith(b'c', 1, 2, 3), 'endswith expected at most 3 arguments, got 4'),
    (lambda: b'abc'.rjust(1, b' ', 3), 'rjust expected at most 2 arguments, got 3'),
    (lambda: b'abc'.lstrip(b' ', b' '), 'lstrip expected at most 1 argument, got 2'),
    (lambda: [1].pop(0, 1), 'pop expected at most 1 argument, got 2'),
    (lambda: [1].index(), 'index expected at least 1 argument, got 0'),
    (lambda: (1,).index(1, 0, 1, 2), 'index expected at most 3 arguments, got 4'),
    (lambda: [1].insert(0), 'insert expected 2 arguments, got 1'),
    (lambda: math.atan2(1), 'atan2 expected 2 arguments, got 1'),
    (lambda: {}.get(), 'get expected at least 1 argument, got 0'),
    (lambda: math.log(), 'log expected at least 1 argument, got 0'),
]:
    try:
        compute()
        assert False, f'expected TypeError: {message}'
    except TypeError as e:
        assert str(e) == message

# A keyword to a positional-only function is reported before the count, with the
# qualified name for a method or a module function.
for compute, message in [
    (lambda: [1].pop(index=0), 'list.pop() takes no keyword arguments'),
    (lambda: [1].pop(0, index=0), 'list.pop() takes no keyword arguments'),
    (lambda: [1].insert(index=0, object=1), 'list.insert() takes no keyword arguments'),
    (lambda: [1].append(object=1), 'list.append() takes no keyword arguments'),
    (lambda: 'a'.strip(chars='a'), 'str.strip() takes no keyword arguments'),
    (lambda: 'a'.join([], x=1), 'str.join() takes no keyword arguments'),
    (lambda: b'a'.strip(bytes=b'a'), 'bytes.strip() takes no keyword arguments'),
    (lambda: {}.get(key=1), 'dict.get() takes no keyword arguments'),
    (lambda: {}.get(1, default=2), 'dict.get() takes no keyword arguments'),
    (lambda: {}.pop(key=1), 'dict.pop() takes no keyword arguments'),
    (lambda: {}.setdefault(key=1), 'dict.setdefault() takes no keyword arguments'),
    (lambda: dict.fromkeys(iterable=[1]), 'dict.fromkeys() takes no keyword arguments'),
    (lambda: {1}.discard(x=1), 'set.discard() takes no keyword arguments'),
    (lambda: math.log(x=1), 'math.log() takes no keyword arguments'),
    (lambda: math.perm(n=1), 'math.perm() takes no keyword arguments'),
    (lambda: math.atan2(y=1, x=1), 'math.atan2() takes no keyword arguments'),
    (lambda: len(obj=1), 'len() takes no keyword arguments'),
    (lambda: divmod(x=1, y=2), 'divmod() takes no keyword arguments'),
    (lambda: list(iterable=[]), 'list() takes no keyword arguments'),
]:
    try:
        compute()
        assert False, f'expected TypeError: {message}'
    except TypeError as e:
        assert str(e) == message
