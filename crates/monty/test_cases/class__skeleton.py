# === Empty class skeletons ===
class Foo:
    pass

assert Foo.__name__ == 'Foo', 'class __name__ should expose the declared class name'
assert repr(Foo) == "<class 'Foo'>", 'class repr should match the datatest harness module context'
assert str(Foo) == "<class 'Foo'>", 'class str should match repr'
assert repr(type(Foo)) == "<class 'type'>", 'type(Foo) should produce the type type object'
assert bool(Foo), 'class objects should be truthy'

# === Distinct class objects ===
class Bar:
    pass

assert Foo != Bar, 'different class definitions should not compare equal'

first_foo = Foo

class Foo:
    pass

assert Foo is not first_foo, 'redefining a class should create a new class object'
assert Foo != first_foo, 'redefined classes should not compare equal'
