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


test_cycle()
# ref-counts={'Foo': 3, 'f': 2, 'g': 1, 'bm': 1}
