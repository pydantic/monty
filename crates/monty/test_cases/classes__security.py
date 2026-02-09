# Security tests for class implementation
# These should trigger errors/limits, NOT crash

# === Recursion Protection ===

# Test: Infinite __init__ recursion
class InfiniteInit:
    def __init__(self):
        InfiniteInit()


try:
    InfiniteInit()
    assert False, 'should have raised RecursionError'
except RecursionError:
    pass


# Test: __init__ mutual recursion
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


# Test: Circular references
class Node:
    def __init__(self):
        self.ref = None


a = Node()
b = Node()
a.ref = b
b.ref = a
# Should not crash when collected


# Test: Class with no methods
class Empty:
    pass


obj = Empty()
assert type(obj).__name__ == 'Empty'


# Test: Instance keeps class alive
def make_instance():
    class Local:
        pass

    return Local()


obj = make_instance()
assert type(obj).__name__ == 'Local'

print('All security tests passed')
