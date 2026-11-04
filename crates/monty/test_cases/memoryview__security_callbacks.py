import gc


class ReleaseIndex:
    def __index__(self):
        view.release()
        owner.clear()
        gc.collect()
        return 0


class ReleaseEq:
    def __eq__(self, other):
        view.release()
        owner.clear()
        gc.collect()
        return answer


for action in ['getitem', 'setkey', 'setvalue', 'index-start', 'index-stop']:
    owner = bytearray(b'abc')
    view = memoryview(owner)
    try:
        if action == 'getitem':
            view[ReleaseIndex()]
        elif action == 'setkey':
            view[ReleaseIndex()] = 0
        elif action == 'setvalue':
            view[0] = ReleaseIndex()
        elif action == 'index-start':
            view.index(97, ReleaseIndex())
        else:
            view.index(97, 0, ReleaseIndex())
    except ValueError as exc:
        assert str(exc) == 'operation forbidden on released memoryview object', (action, str(exc))
    else:
        assert False, action
    assert owner == b''

for length, view_format in [(1, 'B'), (1, 'c'), (3, 'B'), (3, 'c')]:
    for answer in [True, False]:
        for action in ['count', 'index']:
            owner = bytearray(b'a' * length)
            view = memoryview(owner).cast(view_format)
            try:
                result = view.count(ReleaseEq()) if action == 'count' else view.index(ReleaseEq())
            except ValueError as exc:
                result = str(exc)
            if action == 'count' and length == 1:
                expected = int(answer)
            elif action == 'index' and answer:
                expected = 0
            elif action == 'index' and length == 1:
                expected = 'memoryview.index(x): x not found'
            else:
                expected = 'operation forbidden on released memoryview object'
            assert result == expected, (action, length, answer, result)
            assert owner == b''


class LargeFloat:
    def __float__(self):
        view.release()
        owner.clear()
        gc.collect()
        return 1e100


class LargeIndex:
    def __index__(self):
        view.release()
        owner.clear()
        gc.collect()
        return 2**100


for value in [LargeFloat(), LargeIndex()]:
    owner = bytearray(2)
    view = memoryview(owner).cast('e')
    try:
        view[0] = value
    except ValueError as exc:
        assert str(exc) == 'operation forbidden on released memoryview object'
    else:
        assert False
    assert owner == b''


class ReleaseLargeKey:
    def __index__(self):
        view.release()
        owner.clear()
        gc.collect()
        return 2**100


for write in [False, True]:
    owner = bytearray(8)
    view = memoryview(owner)
    try:
        if write:
            view[ReleaseLargeKey()] = 0
        else:
            view[ReleaseLargeKey()]
    except IndexError as exc:
        assert str(exc) == "cannot fit 'ReleaseLargeKey' into an index-sized integer"
    else:
        assert False
    assert owner == b''

# Read mutable contents after converting the grouping count.
for use_view in [False, True]:
    owner = bytearray(b'ab')
    target = memoryview(owner) if use_view else owner

    class HexGroup:
        def __index__(self):
            owner[0] = 90
            return 1

    assert target.hex('-', HexGroup()) == '5a-62'
