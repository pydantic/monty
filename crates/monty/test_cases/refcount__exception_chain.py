# A freed exception releases its `__cause__` and `__context__`, so a chain built
# by `raise ... from` or an implicit context does not outlive the handler.
try:
    try:
        raise ValueError('inner')
    except ValueError as inner:
        raise TypeError('outer') from inner
except TypeError:
    pass

try:
    try:
        raise ValueError('context')
    except ValueError:
        raise KeyError('outer')
except KeyError:
    pass


def deep():
    try:
        try:
            raise ValueError('first')
        except ValueError:
            raise KeyError('second')
    except KeyError as second:
        raise TypeError('third') from second


try:
    deep()
except TypeError as exc:
    kept = exc.__cause__.__context__
assert str(kept) == 'first'
# ref-counts={'kept': 1}
