def local_names(z, a):
    return dir()


assert local_names(1, 2) == ['a', 'z']
assert 'append' in dir([])
assert 'append' in dir(list)

class Base:
    inherited = 1

    @property
    def dangerous(self):
        raise ValueError('enumeration ran a property')


class Child(Base):
    inherited = 2


obj = Child()
vars(obj)['dynamic'] = 3
names = dir(obj)
assert names == sorted(set(names))
assert 'dynamic' in names
assert 'inherited' in names
assert 'dangerous' in names
assert 'dynamic' not in dir(Child)
assert 'inherited' in dir(Child)
assert '__dict__' in dir(Child) and '__class__' in names
delattr(obj, 'dynamic')
assert 'dynamic' not in dir(obj)


class Custom:
    def __dir__(self):
        return ('z', 'a', 'a')


custom = Custom()
custom.__dir__ = lambda: ['wrong']
assert dir(custom) == ['a', 'a', 'z']


class Static:
    @staticmethod
    def __dir__():
        return [2, 1]


assert dir(Static()) == [1, 2]

import math

assert 'sqrt' in dir(math)

class Virtual:
    @property
    def __dict__(self):
        Virtual.added = True
        return {'virtual': 42}


v = Virtual()
v.hidden = True
names = dir(v)
assert 'virtual' in names and 'added' in names and 'hidden' not in names


from collections import namedtuple

Point = namedtuple('Point', ['x', 'y'])
assert 'x' in dir(Point) and 'y' in dir(Point(1, 2))
assert '_make' in dir(Point)

class Invalid:
    def __dir__(self):
        return 42


try:
    dir(Invalid())
except TypeError:
    pass
else:
    assert False, 'dir accepted a non-iterable hook result'
