class Foo:
    def m(self):
        return 1


f = Foo()
g = Foo()
bm = f.m
# Every class carries a synthesized `__annotations__` dict (empty here), which
# is a real heap object owned by the class namespace. Bind it so the strict
# check — every live heap object must be reachable from a name — accounts for
# it rather than seeing an unreferenced object.
ann = Foo.__annotations__
# ref-counts={'Foo': 3, 'f': 2, 'g': 1, 'bm': 1, 'ann': 2}
