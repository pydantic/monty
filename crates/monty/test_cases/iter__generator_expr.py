# === Basic generator expression ===
gen = (x * 2 for x in range(5))
assert type(gen).__name__ == 'generator'
assert iter(gen) is gen
assert type(hash(gen)) is int
assert gen == gen
assert gen != (x for x in range(5))
assert next(gen) == 0
assert next(gen) == 2
assert list(gen) == [4, 6, 8]
assert next(gen, 42) == 42
try:
    next(gen)
    assert False, 'expected exhausted generator'
except StopIteration:
    pass
try:
    next(gen)
    assert False, 'expected sticky generator exhaustion'
except StopIteration:
    pass

# === Consumers ===
assert list(x * 2 for x in range(5)) == [0, 2, 4, 6, 8]
assert tuple(x for x in range(3)) == (0, 1, 2)
assert sum(x for x in range(5)) == 10
assert ''.join(str(x) for x in range(3)) == '012'
a, b, c = (x + 1 for x in range(3))
assert (a, b, c) == (1, 2, 3)

# === Filters, nested clauses, and unpacking ===
assert list(x for x in range(10) if x % 2 == 0) == [0, 2, 4, 6, 8]
assert list(x + y for x in range(3) for y in range(2)) == [0, 1, 1, 2, 2, 3]
pairs = [(1, 2), (3, 4)]
assert list(a + b for a, b in pairs) == [3, 7]

# === Eager outer iterable and lazy body ===
events = []


def make_outer():
    events.append('outer')
    return [1, 2]


def transform(value):
    events.append(value)
    return value * 10


lazy = (transform(x) for x in make_outer())
assert events == ['outer']
assert next(lazy) == 10
assert events == ['outer', 1]
assert list(lazy) == [20]
assert events == ['outer', 1, 2]


class IterationSource:
    def __iter__(self):
        events.append('iter')
        return iter([3])


custom_source = (x for x in IterationSource())
assert events == ['outer', 1, 2, 'iter']
assert next(custom_source) == 3

source = [1]
captured_iterator = (x for x in source)
source = [2]
assert list(captured_iterator) == [1]

try:
    invalid = (x for x in 1)
    assert False, 'expected generator creation to reject a non-iterable'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# A later iterable is not evaluated until its outer item is requested.
events = []


def make_inner(value):
    events.append(value)
    return range(value)


lazy_inner = (y for x in [2, 3] for y in make_inner(x))
assert events == []
assert next(lazy_inner) == 0
assert events == [2]
assert next(lazy_inner) == 1
assert next(lazy_inner) == 0
assert events == [2, 3]


# === Enclosing closures use late binding ===
def late_binding():
    value = 1
    generator = (value + x for x in [1])
    value = 10
    return next(generator)


assert late_binding() == 11


# The first iterable sees class locals, while the deferred body skips class scope.
class GeneratorClassScope:
    values = [1]
    generator = (values for _ in values)


try:
    next(GeneratorClassScope.generator)
    assert False, 'expected lazy generator body to skip class scope'
except NameError as exc:
    assert str(exc) == "name 'values' is not defined"

# === Nested child scopes capture one stable target cell ===
outer = ((x for _ in [0]) for x in [1, 2])
first = next(outer)
second = next(outer)
assert list(first) == [2]
assert list(second) == [2]

# Lambdas created by the body retain the same target cell.
functions = list((lambda: x) for x in [1, 2])
assert functions[0]() == 2
assert functions[1]() == 2

# === Walrus targets bind in the nearest real scope ===
result = 99
generators = (((result := x) for x in range(y + 1)) for y in range(2))
first = next(generators)
assert result == 99
assert list(first) == [0]
assert result == 0
assert list(next(generators)) == [0, 1]
assert result == 1


def function_walrus():
    result = 10
    before = result
    generator = (result := x for x in [1])
    still_before = result
    yielded = next(generator)
    return before, still_before, yielded, result


assert function_walrus() == (10, 10, 1, 1)


# === Escaping errors close the generator ===
def fail(value):
    raise ValueError(str(value))


failed = (fail(x) for x in [1, 2])
try:
    next(failed)
    assert False, 'expected lazy body error'
except ValueError as exc:
    assert str(exc) == '1'
try:
    next(failed)
    assert False, 'expected failed generator to stay closed'
except StopIteration:
    pass

# === Re-entry is rejected and closes the outer execution ===
reentrant = (next(reentrant) for _ in [0])
try:
    next(reentrant)
    assert False, 'expected generator re-entry error'
except ValueError as exc:
    assert str(exc) == 'generator already executing'
try:
    next(reentrant)
    assert False, 'expected re-entry failure to close generator'
except StopIteration:
    pass


# A re-entry error caught by the lazy body does not close the active generator.
def catch_reentry():
    try:
        next(caught_reentry)
    except ValueError as exc:
        assert str(exc) == 'generator already executing'
        return 1
    return 0


caught_reentry = (catch_reentry() for _ in [0])
assert next(caught_reentry) == 1
try:
    next(caught_reentry)
    assert False, 'expected caught-reentry generator exhaustion'
except StopIteration:
    pass


# === PEP 479 ===
def raises_stop_iteration():
    raise StopIteration('escaped')


pep479 = (raises_stop_iteration() for _ in [0])
try:
    next(pep479)
    assert False, 'expected PEP 479 conversion'
except RuntimeError as exc:
    assert str(exc) == 'generator raised StopIteration'
try:
    next(pep479)
    assert False, 'expected PEP 479 failure to close generator'
except StopIteration:
    pass
