class Foo:
    def m(self):
        return 1


f = Foo()
g = Foo()
bm = f.m
# ref-counts={'Foo': 3, 'f': 2, 'g': 1, 'bm': 1}
