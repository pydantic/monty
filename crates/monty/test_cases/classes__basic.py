# Basic class definition and instantiation


class Dog:
    species = 'Canis familiaris'

    def __init__(self, name, age):
        self.name = name
        self.age = age

    def bark(self):
        return f'{self.name} says woof'

    def get_age(self):
        return self.age


d = Dog('Rex', 5)
assert d.name == 'Rex', f"expected 'Rex', got {d.name}"
assert d.age == 5, f'expected 5, got {d.age}'
assert d.bark() == 'Rex says woof', f'unexpected bark: {d.bark()}'
assert Dog.species == 'Canis familiaris', 'class attr access failed'
assert d.species == 'Canis familiaris', 'instance class attr access failed'

# Multiple instances are independent
d2 = Dog('Buddy', 3)
assert d2.name == 'Buddy'
assert d.name == 'Rex'

# type() returns the class
assert type(d) is Dog, f'type mismatch: {type(d)}'

# Instance attribute shadows class attribute
d.species = 'wolf'
assert d.species == 'wolf', 'instance shadow failed'
assert Dog.species == 'Canis familiaris', 'class attr mutated'


# Class with no __init__
class Empty:
    pass


e = Empty()
assert type(e) is Empty


# Class attribute mutation via method
class Counter:
    count = 0

    def __init__(self):
        c = Counter.count
        Counter.count = c + 1


Counter()
Counter()
assert Counter.count == 2, f'expected 2, got {Counter.count}'


# Method returning self (chaining)
class Builder:
    def __init__(self):
        self.parts = []

    def add(self, part):
        self.parts.append(part)
        return self


b = Builder()
b.add('a').add('b').add('c')
assert b.parts == ['a', 'b', 'c'], f'chaining failed: {b.parts}'


# Circular references don't crash
class Node:
    def __init__(self):
        self.next = None


a = Node()
b = Node()
a.next = b
b.next = a  # Circular reference — should not crash
