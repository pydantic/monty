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
    random.getrandbits(k=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Random.getrandbits() takes no keyword arguments'
try:
    random.getrandbits(2**70)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large for C uint64_t'
try:
    random.randbytes(2**70)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large for C uint64_t'
try:
    random.randbytes(2**63 - 1)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large for C uint64_t'
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
# len() of a range beyond ssize_t overflows before any draw
for fn in (random.choice, random.shuffle, lambda r: random.choices(r, k=1), lambda r: random.sample(r, 1)):
    try:
        fn(range(-(2**63), 2**63 - 1))
        assert False, 'expected OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'Python int too large to convert to C ssize_t'
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
try:
    random.shuffle({1, 2})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'set' object is not subscriptable"
# a dict keyed 0..n-1 shuffles through __getitem__/__setitem__ like any sequence
d = {0: 'a', 1: 'b', 2: 'c', 3: 'd'}
random.Random(3).shuffle(d)
letters = ['a', 'b', 'c', 'd']
random.Random(3).shuffle(letters)
assert [d[0], d[1], d[2], d[3]] == letters
try:
    # seed 1 draws index 0 for the single swap, which is not a key
    random.Random(1).shuffle({1: 'a', 5: 'b'})
    assert False, 'expected KeyError'
except KeyError as exc:
    # Monty's dict KeyError carries the key's str(), see exceptions.md
    assert str(exc) in ('0', "'0'")

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

# The virtual population must fit in a sequence length even when each count does.
for counts in ([2**63 - 1, 1], [2**63 - 1] * 3):
    try:
        random.sample(list(range(len(counts))), 1, counts=counts)
        assert False, 'expected OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'Python int too large to convert to C ssize_t'
assert random.sample(['a'], 1, counts=[2**63 - 1]) == ['a']

# === choices() ===
for k in (5, True, -5, 2**70):
    try:
        random.choices([1, 2], k)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f'The number of choices must be a keyword argument: k={k}'
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
# a NaN p passes the range check and fails the BTRS precondition assertion
try:
    random.binomialvariate(2, float('nan'))
    assert False, 'expected AssertionError'
except AssertionError as exc:
    assert str(exc) == ''
assert random.binomialvariate(1, float('nan')) == 0
assert 1 <= random.triangular(low=1, high=2, mode=1.5) <= 2
# low == high with a mode returns low itself, int included
assert type(random.triangular(5, 5, 5)) is int
assert type(random.triangular(5, 5)) is float
assert random.gauss(mu=1, sigma=0) == 1.0

# Shared parameter shapes must still name the function actually called.
for name in ('gauss', 'betavariate', 'weibullvariate'):
    try:
        getattr(random, name)(unknown=1)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f"Random.{name}() got an unexpected keyword argument 'unknown'"

try:
    random.lognormvariate(1000, 0)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'math range error'

# CPython's float-power overflow wording comes from the host libc.
for name, args in [('paretovariate', (0.00001,)), ('weibullvariate', (1, 0.00001))]:
    random.seed(0)
    try:
        getattr(random, name)(*args)
        assert False, 'expected OverflowError'
    except OverflowError as exc:
        assert str(exc) in {"(34, 'Numerical result out of range')", "(34, 'Result too large')"}

# Infinite inputs or exponents propagate; they are not finite-input overflow.
assert random.lognormvariate(float('inf'), 0) == float('inf')
assert random.weibullvariate(float('inf'), 1) == float('inf')
random.seed(0)
assert random.paretovariate(1e-310) == float('inf')
random.seed(0)
assert random.weibullvariate(1, 1e-310) == float('inf')

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
# the unpack stops at the fourth item; only list, tuple and dict report a total
try:
    random.setstate(b'\x03\x00\x00\x00')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'too many values to unpack (expected 3)'
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
# a version 2 state is reduced with `%` first, so its errors are the operator's
try:
    random.setstate((2, ('a',) * 624 + (0,), None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'not all arguments converted during string formatting'
try:
    random.setstate((2, (None,) * 624 + (0,), None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for %: 'NoneType' and 'int'"
try:
    random.setstate((2, (1.5,) * 624 + (0,), None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'an integer is required'
try:
    random.setstate((2, 7, None))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

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
