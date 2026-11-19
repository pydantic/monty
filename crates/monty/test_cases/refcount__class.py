class Foo:
    def m(self):
        return 1


f = Foo()
g = Foo()
bm = f.m


def test_cycle():
    class Base:
        pass

    class Child(Base):
        pass

    Base.child = Child


def test_dynamic_cycle():
    """Exercise base/child cycles with retained heap-backed class names."""
    base_name = ''.join(['Dynamic', 'BaseClassName'])
    child_name = ''.join(['Dynamic', 'ChildClassName'])
    Base = type(base_name, (), {})
    Child = type(child_name, (Base,), {})
    Base.child = Child
    assert Base.__name__ is base_name
    assert Child.__name__ is child_name
    assert Child.__bases__ == (Base,)
    base_name = child_name = None
    assert Base.__name__ == 'DynamicBaseClassName'
    assert Child.__name__ == 'DynamicChildClassName'


test_cycle()
test_dynamic_cycle()
# ref-counts={'Foo': 3, 'f': 2, 'g': 1, 'bm': 1}
