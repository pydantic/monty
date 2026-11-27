class Base:
    inherited = 7


class Child(Base):
    count = 1


obj = Child()
attrs = vars(obj)
assert attrs is obj.__dict__
attrs['value'] = 42
assert obj.value == 42
obj.value = 9
assert attrs == {'value': 9}

members = vars(Child)
Child.count = 2
assert members['count'] == 2
assert 'inherited' not in members
delattr(Child, 'count')
assert 'count' not in members
other = {'new': 3}
obj.__dict__ = other
assert vars(obj) is other
assert obj.new == 3
delattr(obj, '__dict__')
assert vars(obj) == {}
assert attrs == {'value': 9}
assert other == {'new': 3}


def check_local(value):
    assert vars() == {'value': value}


check_local(12)


class Custom:
    @property
    def __dict__(self):
        return 42


assert vars(Custom()) == 42
assert vars(dict)['fromkeys'] is dict.__dict__['fromkeys']
try:
    vars(42)
except TypeError as exc:
    assert str(exc) == 'vars() argument must have __dict__ attribute'
else:
    assert False
