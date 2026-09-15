# Seeded `random`: every value here is CPython 3.14's for the same seed, so
# the fixture pins the MT19937 core, the seeding and the draw order of each
# method. Nothing draws from an unseeded generator (that would need the host).
import random

# === random() / getrandbits() / randbytes() ===
random.seed(42)
assert random.random() == 0.6394267984578837
assert random.random() == 0.025010755222666936
assert random.getrandbits(5) == 8
assert random.getrandbits(32) == 1051802512
assert random.getrandbits(33) == 958682846
assert random.getrandbits(64) == 1890702223848595625
assert random.getrandbits(100) == 704511047883023533822611041645
assert random.randbytes(0) == b''
assert random.randbytes(1) == b'\x16'
assert random.randbytes(7) == b'i\x84*\x97\x11\x03l'

# === randrange() / randint() ===
assert random.randrange(10) == 0
assert random.randrange(5, 10) == 5
assert random.randrange(0, 100, 7) == 7
assert random.randrange(10, 0, -3) == 7
assert random.randrange(-50, 50) == -21
assert random.randint(1, 6) == 5
assert random.randint(-10, 10) == 9
assert random.randint(0, 0) == 0
assert random.randint(0, 2**62) == 1833953597603037606

# === choice() / shuffle() ===
assert random.choice([1, 2, 3, 4]) == 4
assert random.choice('abcdef') == 'b'
assert random.choice((10, 20)) == 20
assert random.choice(range(100)) == 75
x = list(range(20))
assert random.shuffle(x) is None
assert x == [12, 7, 18, 11, 14, 19, 16, 9, 6, 15, 1, 17, 3, 2, 4, 10, 13, 5, 0, 8]

# === sample(): the pool branch (small populations) and the set branch ===
assert random.sample(range(100), 3) == [68, 15, 48]
assert random.sample([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 8) == [2, 9, 5, 7, 6, 8, 3, 4]
assert random.sample(range(1000), 50) == [
    196, 721, 71, 46, 677, 233, 791, 296, 81, 875, 238, 887, 103, 389, 284, 464, 650, 854, 373, 166, 379, 363, 214,
    686, 273, 718, 959, 699, 663, 73, 623, 175, 546, 746, 250, 167, 473, 388, 276, 947, 655, 704, 570, 224, 701, 332,
    863, 786, 794, 57,
]  # fmt: skip
assert random.sample('abcdef', k=2) == ['b', 'a']
assert random.sample(['red', 'blue'], counts=[4, 2], k=5) == ['red', 'red', 'blue', 'red', 'blue']

# === choices() ===
assert random.choices([1, 2, 3], k=5) == [3, 2, 3, 1, 2]
assert random.choices('abc', weights=[1, 2, 3], k=6) == ['c', 'c', 'a', 'a', 'c', 'c']
assert random.choices([1, 2, 3], cum_weights=[1.0, 1.5, 4.0], k=4) == [3, 3, 3, 2]
assert random.choices([1, 2], k=0) == []
assert random.choices([1, 2], k=-3) == []


# === real-valued distributions: same libm on the same platform, so compare
# with a tolerance rather than bit for bit ===
def close(a, b):
    return abs(a - b) < 1e-9


assert close(random.uniform(1, 10), 9.975932001902448)
assert close(random.uniform(-2.5, 2.5), -1.8083412755457207)
assert close(random.triangular(), 0.4967473376323375)
assert close(random.triangular(0, 10, 3), 5.865359873865882)
assert random.triangular(5, 5) == 5.0
assert close(random.normalvariate(), -0.7089852341837076)
assert close(random.normalvariate(10, 2), 11.53432835359824)
assert close(random.gauss(), -1.0084618341434546)
assert close(random.gauss(), 0.8917024573962434)
assert close(random.gauss(1, 3), -1.2371860605146665)
assert close(random.lognormvariate(0, 1), 4.884669381138477)
assert close(random.expovariate(), 1.140320332665912)
assert close(random.expovariate(2.5), 0.04866450284057625)
assert close(random.vonmisesvariate(0, 0), 5.559289789813416)
assert close(random.vonmisesvariate(1, 4), 0.706807261253072)
assert close(random.gammavariate(2, 3), 0.21983147098351846)
assert close(random.gammavariate(1, 2), 2.543651714776456)
assert close(random.gammavariate(0.5, 1), 0.3612785331332452)
assert close(random.betavariate(2, 3), 0.4691619300052024)
assert close(random.paretovariate(2), 1.2638133906427838)
assert close(random.weibullvariate(1, 1.5), 0.31429387793192437)
assert random.binomialvariate() == 0
assert random.binomialvariate(5, 0.3) == 3
assert random.binomialvariate(100, 0.4) == 33
assert random.binomialvariate(100, 0.9) == 87
assert random.binomialvariate(5, 0) == 0
assert random.binomialvariate(5, 1) == 5
assert random.binomialvariate(0, 0.5) == 0

# === seed types: ints use abs(), floats hash, str/bytes are sha512-extended ===
random.seed(0)
assert random.random() == 0.8444218515250481
assert random.getrandbits(40) == 978212965548
random.seed(1)
assert random.random() == 0.13436424411240122
random.seed(True)
assert random.random() == 0.13436424411240122
random.seed(-42)
assert random.random() == 0.6394267984578837
random.seed(2**64 + 5)
assert random.random() == 0.5105783769365112
assert random.getrandbits(40) == 626548792571
random.seed(3.25)
assert random.random() == 0.10626827177291942
random.seed(-1.5)
assert random.random() == 0.7101153317194698
random.seed('hello')
assert random.random() == 0.3537754404730722
assert random.getrandbits(40) == 836072071631
random.seed(b'bytes')
assert random.random() == 0.37075677971469856
random.seed('')
assert random.random() == 0.9602256525641875
random.seed('hello', version=1)
assert random.random() == 0.8180391270568783
random.seed(b'hi', version=1)
assert random.random() == 0.849748721539462
random.seed('hello', version=1.0)
assert random.random() == 0.8180391270568783
random.seed(a=7, version=2)
assert random.random() == random.Random(7).random()

# Other versions use the seed's unsigned hash, which differs between interpreters.
for seed_value in ('hello', b'bytes', '', b'', ''.join(['he', 'llo']), b''.join([b'by', b'tes'])):
    expected = random.Random(hash(seed_value) % 2**64).getstate()
    rng = random.Random(0)
    for version in (0, -1, 3, 2**100, 0.0, 1.5, 3.0, False, None, '2', [], {}):
        random.seed(seed_value, version=version)
        assert random.getstate() == expected, (seed_value, version)
        rng.seed(seed_value, version=version)
        assert rng.getstate() == expected, (seed_value, version)

# === Random instances have their own state ===
r = random.Random(7)
assert r.random() == 0.32383276483316237
assert r.randint(1, 100) == 20
assert r.choice('xyz') == 'y'
assert isinstance(r, random.Random)
assert type(r) is random.Random
assert repr(r).startswith('<random.Random object at 0x')
assert r.VERSION == 3
assert random.Random(x=5).random() == random.Random(5).random()
random.seed(7)
assert random.random() == 0.32383276483316237

# === getstate() / setstate() ===
r2 = random.Random(7)
state = r2.getstate()
assert state[0] == 3
assert len(state[1]) == 625
assert state[1][-1] == 624
assert state[2] is None
first = r2.random()
r2.setstate(state)
assert r2.random() == first
assert random.Random(7).getstate() == state
random.seed(1)
before = random.getstate()
random.gauss()
after = random.getstate()
assert before[2] is None
assert type(after[2]) is float
random.setstate(before)
assert random.gauss() == random.Random(1).gauss()
# a version 2 state (signed words) restores like a version 3 one
words = list(state[1][:-1])
signed = tuple(w - 2**32 if w >= 2**31 else w for w in words) + (state[1][-1],)
r3 = random.Random()
r3.setstate((2, signed, None))
assert r3.random() == random.Random(7).random()
# version 2 accepts any iterable of words, and big ints reduce modulo 2**32
r3.setstate((2, [w + 2**40 for w in signed], None))
assert r3.random() == random.Random(7).random()
# a list outer state works as well as a tuple
r3.setstate([3, state[1], 0.5])
assert r3.getstate()[2] == 0.5
