# Tests exception creation, overwriting, and chaining without leaking references.


def test_cycles():
    # Basic creation and scope release of exception instances
    for _ in range(10):
        e = ValueError('msg')
        e = None

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

    # Exception self-referential attribute cycle
    for _ in range(10):
        e = ValueError('cycle')
        e.cycle = e
        e = None

    # Exception list cycle
    for _ in range(10):
        e = ValueError('list_cycle')
        lst = [e]
        e.lst = lst
        e = None
        lst = None

    # Exception self-referential args cycle
    for _ in range(10):
        e = ValueError('args_cycle')
        e.args = (e,)
        e = None

    # Exception self-referential cause cycle (raise e from e)
    for _ in range(10):
        try:
            e = ValueError('self_cause')
            raise e from e
        except ValueError:
            pass

    # Mutual cause cycle (e1 from e2, e2 from e1)
    for _ in range(10):
        e1 = ValueError('e1')
        e2 = ValueError('e2')
        e1.__cause__ = e2
        e2.__cause__ = e1
        e1 = None
        e2 = None

    # Mutual context cycle (e1.__context__ = e2, e2.__context__ = e1)
    for _ in range(10):
        e1 = ValueError('ctx1')
        e2 = ValueError('ctx2')
        e1.__context__ = e2
        e2.__context__ = e1
        e1 = None
        e2 = None

    # Mixed chain + attribute cycle
    for _ in range(10):
        e1 = ValueError('mixed1')
        e2 = ValueError('mixed2')
        e1.attr = e2
        e2.__cause__ = e1
        e1 = None
        e2 = None


test_cycles()
test_cycles = None

# ref-counts={}
