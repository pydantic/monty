"""__new__ allocates; __init__ initializes the returned instance."""

events = []


class Record:
    def __new__(cls, name):
        events.append('new')
        instance = object.__new__(cls)
        instance.created_as = cls
        return instance

    def __init__(self, name):
        events.append('init')
        self.name = name


class NamedRecord(Record):
    pass


record = NamedRecord('example')
assert record.created_as is NamedRecord
assert record.name == 'example'
assert events == ['new', 'init']
print('Construction order:', events)


class Answer:
    def __new__(cls):
        return 42

    def __init__(self):
        raise AssertionError('__init__ should be skipped')


assert Answer() == 42
print('__new__ can return another type:', Answer())
