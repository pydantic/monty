# Tests exception creation, overwriting, and chaining without leaking references.

# Basic creation and scope release of exception instances
def make_exceptions():
    for _ in range(10):
        e = ValueError('msg')
        e = None

make_exceptions()

# Exception with chained cause
for _ in range(10):
    try:
        try:
            raise ValueError('inner')
        except ValueError as inner:
            raise RuntimeError('outer') from inner
    except RuntimeError:
        pass

# Exception with chained context
for _ in range(10):
    try:
        try:
            raise ValueError('first')
        except ValueError:
            raise TypeError('second')
    except TypeError:
        pass
