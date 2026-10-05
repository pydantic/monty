# xfail=monty
# Follow-up: explicit and zero-argument super() preserve the instance receiver.


class Parent:
    def __init__(self, value):
        self.value = value

    def read(self):
        return self.value


class Child(Parent):
    def __init__(self, value):
        super().__init__(value + 1)

    def read(self):
        return super().read() * 2

    def explicit_read(self):
        return super(Child, self).read()


class Grandchild(Child):
    def read(self):
        return super().read() + 10


child = Child(3)
grandchild = Grandchild(3)
assert child.read() == 8
assert child.explicit_read() == 4
assert grandchild.read() == 18
assert grandchild.explicit_read() == 4
assert super(Grandchild, grandchild).read() == 8
assert super(Child, grandchild).read() == 4
