class Foo:
    @staticmethod
    def bar():
        return 42


assert Foo.bar() == 42
