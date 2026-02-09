class Foo:
    @property
    def bar(self):
        return 42


f = Foo()
assert f.bar == 42
