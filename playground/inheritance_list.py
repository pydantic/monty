"""Custom attributes and methods share the instance's native list storage."""


class LoggedList(list):
    def __init__(self, values):
        list.__init__(self, values)
        self.appended = []

    def append(self, value):
        self.appended.append(value)
        list.append(self, value)


class Numbers(LoggedList):
    pass


numbers = Numbers([3, 1])
numbers.append(2)
# Calling the native method directly bypasses the override.
list.append(numbers, 4)
assert numbers.appended == [2]
assert numbers == [3, 1, 2, 4]
assert isinstance(numbers, list)
assert isinstance(numbers, LoggedList)
assert issubclass(Numbers, list)
assert Numbers.__bases__ == (LoggedList,)

numbers[0] = 5
numbers.sort()
assert list(numbers) == [1, 2, 4, 5]
assert 4 in numbers
assert len(numbers) == 4
assert type(numbers[:]) is list
assert type(numbers.copy()) is list
print('List contents:', numbers)
print('Calls to the override:', numbers.appended)


class EmptyList(list):
    def __init__(self, values):
        self.requested = values


# Allocation supplies empty storage even when __init__ never calls list.__init__.
empty = EmptyList([9])
assert empty == []
assert empty.requested == [9]
empty.append(7)
assert empty == [7]
print('Storage exists before native initialization:', empty)
