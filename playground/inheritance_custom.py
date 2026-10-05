"""Inherited initialization, methods, class attributes, and dynamic classes."""


class Counter:
    step = 1

    def __init__(self, value):
        self.value = value

    def increment(self):
        self.value += self.step
        return self.value


class FastCounter(Counter):
    step = 5


class FasterCounter(FastCounter):
    step = 10


counter = FasterCounter(2)
assert counter.increment() == 12
assert type(counter) is FasterCounter
assert isinstance(counter, Counter)
assert issubclass(FasterCounter, Counter)
assert FasterCounter.__bases__ == (FastCounter,)

# A changed parent method is visible to existing child instances.
Counter.increment = lambda self: self.value + 100
assert counter.increment() == 112

DynamicCounter = type('DynamicCounter', (Counter,), {'step': 3})
dynamic = DynamicCounter(7)
assert dynamic.increment() == 107
print('Custom inheritance:', counter.value, dynamic.value)
