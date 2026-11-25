# Tests custom exception creation, overwriting, and cycles without leaking references.


class MyError(Exception):
    pass


class SubError(MyError):
    pass


def test_custom_exception_cycles():
    # Basic creation and scope release of custom exception instances
    for _ in range(10):
        e = MyError('msg')
        assert e.args == ('msg',)
        e = None

    # Custom exception self-referential attribute cycle
    for _ in range(10):
        e = MyError('self_attr')
        e.self_ref = e
        assert e.self_ref is e
        e = None

    # Custom exception self-referential args cycle
    for _ in range(10):
        e = MyError('args_cycle')
        e.args = (e,)
        assert e.args[0] is e
        e = None

    # Custom exception list cycle
    for _ in range(10):
        e = SubError('list_cycle')
        lst = [e]
        e.lst = lst
        assert e.lst[0] is e
        e = None
        lst = None

    # Custom exception chained cause cycle
    for _ in range(10):
        e1 = MyError('e1')
        e2 = SubError('e2')
        e1.__cause__ = e2
        e2.__cause__ = e1
        assert e1.__cause__ is e2
        assert e2.__cause__ is e1
        e1 = None
        e2 = None


test_custom_exception_cycles()
test_custom_exception_cycles = None
MyError = None
SubError = None
