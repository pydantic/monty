class InfiniteInit:
    def __init__(self):
        InfiniteInit()


try:
    InfiniteInit()
    assert False, 'should have raised RecursionError'
except RecursionError:
    pass

print('caught class recursion')
