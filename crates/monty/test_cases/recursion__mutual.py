class MutA:
    def __init__(self):
        MutB()


class MutB:
    def __init__(self):
        MutA()


try:
    MutA()
    assert False, 'should have raised RecursionError'
except RecursionError:
    pass

print('caught mutual recursion')
