def exercise(operation, match=False, size=3):
    owner = bytearray(b'abc'[:size])
    view = memoryview(owner)

    class ReleasingIndex:
        def __index__(self):
            view.release()
            owner.clear()
            return 0

    class ReleasingEqual:
        def __eq__(self, other):
            view.release()
            owner.clear()
            return match

    try:
        if operation == 'get':
            result = view[ReleasingIndex()]
        elif operation == 'set_index':
            view[ReleasingIndex()] = 1
            result = None
        elif operation == 'set_value':
            view[0] = ReleasingIndex()
            result = None
        elif operation == 'count':
            result = view.count(ReleasingEqual())
        elif operation == 'index':
            result = view.index(ReleasingEqual())
        elif operation == 'start':
            result = view.index(97, ReleasingIndex())
        else:
            result = view.index(97, 0, ReleasingIndex())
    except Exception as exc:
        result = (type(exc).__name__, str(exc))
    return result, bytes(owner)


for operation in ['get', 'set_index', 'set_value', 'count', 'index', 'start', 'stop']:
    assert exercise(operation) == (('ValueError', 'operation forbidden on released memoryview object'), b''), operation

for matched in [False, True]:
    assert exercise('count', matched, 1) == (int(matched), b'')
    assert exercise('index', True, 3) == (0, b'')
assert exercise('index', False, 1) == (('ValueError', 'memoryview.index(x): x not found'), b'')
