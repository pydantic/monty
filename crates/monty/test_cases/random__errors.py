# Error paths of `random`, with CPython 3.14's messages.
import random

random.seed(1)

# === seed() ===
for bad in ([1], {}, (1,), range(3)):
    try:
        random.seed(bad)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'The only supported seed types are:\nNone, int, float, str, bytes, and bytearray.'

# === getrandbits() / randbytes() ===
assert random.getrandbits(0) == 0
assert random.getrandbits(True) in (0, 1)
try:
    random.getrandbits(-1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Cannot convert negative int'
try:
    random.getrandbits(1.5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"
try:
    random.getrandbits('a')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"
try:
    random.randbytes(-1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Cannot convert negative int'
try:
    random.randbytes(1.5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"

# === randrange() / randint() ===
try:
    random.randrange(0)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range for randrange()'
try:
    random.randrange(-5)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range for randrange()'
try:
    random.randrange(5, 5)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range in randrange(5, 5)'
try:
    random.randrange(True, False)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range in randrange(True, False)'
try:
    random.randrange(0, 10, 0)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'zero step for randrange()'
try:
    random.randrange(0, 10, -1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range in randrange(0, 10, -1)'
try:
    random.randrange(10, 0, 2)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range in randrange(10, 0, 2)'
try:
    random.randrange(1.5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"
try:
    random.randrange(0, 2.5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"
try:
    random.randrange(10, None, 2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Missing a non-None stop argument'
try:
    random.randrange(10, None, True)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Missing a non-None stop argument'
# an explicit step of 1 is the default object in CPython, so no error
assert 0 <= random.randrange(10, None, 1) < 10
assert 1 <= random.randrange(start=1, stop=5) < 5
try:
    random.randint(1.5, 3)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"
try:
    random.randint(5, 1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'empty range in randint(5, 1)'
assert random.randint(False, True) in (0, 1)

# === choice() / shuffle() ===
try:
    random.choice([])
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'Cannot choose from an empty sequence'
try:
    random.choice('')
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'Cannot choose from an empty sequence'
try:
    random.choice(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'int' has no len()"
try:
    random.choice({1, 2})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'set' object is not subscriptable"
assert random.choice(range(3, 4)) == 3
try:
    random.shuffle((1, 2, 3))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'tuple' object does not support item assignment"
assert random.shuffle((1,)) is None
try:
    random.shuffle('abc')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object does not support item assignment"
try:
    random.shuffle(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'int' has no len()"

# === sample() ===
for population in ({1, 2, 3}, {1: 2}):
    try:
        random.sample(population, 1)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'Population must be a sequence.  For dicts or sets, use sorted(d).'
for k in (3, -1):
    try:
        random.sample([1, 2], k)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'Sample larger than population or is negative'
try:
    random.sample([1, 2], counts=[1], k=1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'The number of counts does not match the population'
try:
    random.sample([1, 2], counts=[1, -5], k=1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Counts must be non-negative'
try:
    random.sample([1, 2], counts=[1.5, 2], k=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Counts must be integers'
assert random.sample([1, 2], counts=[0, 0], k=0) == []
assert random.sample([], 0) == []

# === choices() ===
try:
    random.choices([1, 2], 5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'The number of choices must be a keyword argument: k=5'
try:
    random.choices([1, 2], weights=[1, 1], cum_weights=[1, 2])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Cannot specify both weights and cumulative weights'
try:
    random.choices([1, 2], weights=[1])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'The number of weights does not match the population'
for weights in ([0, 0], [-1, 0]):
    try:
        random.choices([1, 2], weights=weights)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'Total of weights must be greater than zero'
try:
    random.choices([1, 2], weights=[1, float('inf')])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Total of weights must be finite'
try:
    random.choices([])
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'list index out of range'
try:
    random.choices([], weights=[])
    assert False, 'expected IndexError'
except IndexError as exc:
    assert str(exc) == 'list index out of range'
try:
    random.choices([1, 2], k=1.5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'float' object cannot be interpreted as an integer"
try:
    random.choices(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'int' has no len()"
assert len(random.choices([1, 2], k=True)) == 1

# === distributions ===
try:
    random.expovariate(0)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'
assert random.expovariate(-2) < 0
for alpha, beta in ((0, 1), (1, -1)):
    try:
        random.gammavariate(alpha, beta)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'gammavariate: alpha and beta must be > 0.0'
try:
    random.betavariate(0, 1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'gammavariate: alpha and beta must be > 0.0'
try:
    random.paretovariate(0)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'
try:
    random.weibullvariate(1, 0)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as exc:
    assert str(exc) == 'division by zero'
try:
    random.binomialvariate(-1)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'n must be non-negative'
try:
    random.binomialvariate(5, 2)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'p must be in the range 0.0 <= p <= 1.0'
assert 0 <= random.binomialvariate(n=3, p=0.5) <= 3
assert 1 <= random.triangular(low=1, high=2, mode=1.5) <= 2
assert random.gauss(mu=1, sigma=0) == 1.0

# === setstate() ===
try:
    random.setstate(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not subscriptable"
try:
    random.setstate((1, (), None))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'state with version 1 passed to Random.setstate() of version 3'
try:
    random.setstate((3,))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'not enough values to unpack (expected 3, got 1)'
try:
    random.setstate((3, 1, 2, 3))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'too many values to unpack (expected 3, got 4)'
try:
    random.setstate((3, [1] * 625, None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'state vector must be a tuple'
try:
    random.setstate((3, (1, 2), None))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'state vector is the wrong size'
try:
    random.setstate((3, (-1,) * 624 + (0,), None))
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == "can't convert negative value to unsigned int"
try:
    random.setstate((3, ('a',) * 624 + (0,), None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'an integer is required'
for index in (700, -1):
    try:
        random.setstate((3, (1,) * 624 + (index,), None))
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'invalid state'
try:
    random.setstate((3, (1,) * 624 + ('x',), None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"
try:
    random.setstate((3, (2**70,) * 624 + (0,), None))
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C unsigned long'

# === attributes ===
try:
    random.Random(1).foo
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'Random' object has no attribute 'foo'"
try:
    len(random.Random(1))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'Random' has no len()"
