# `random` module

Monty implements `random` on CPython's own Mersenne Twister core, ported from
`Modules/_randommodule.c`, with the pure-Python method bodies of `Lib/random.py`
ported 1:1 on top. A seeded generator therefore produces exactly CPython 3.14's
sequence: `random.seed(42)` gives the same `random()`, `randint()`, `choice()`,
`shuffle()`, `sample()` and `choices()` results as CPython, and the same
distribution values on the same platform (see the notes on floats below).

## Implemented

- **Module functions:** `random`, `seed`, `getstate`, `setstate`, `getrandbits`,
    `randbytes`, `randrange`, `randint`, `choice`, `choices`, `shuffle`, `sample`,
    `uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`,
    `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`,
    `paretovariate`, `weibullvariate`, `binomialvariate`.
- **`random.Random(x=None)`** instances with the same methods and an
    independent state, plus `instance.VERSION`.

## Entropy comes from the host

An unseeded generator has no entropy of its own. The first draw from the
module-level generator, or from a `random.Random()` created without a seed,
suspends with an `os.urandom` host call for 2496 bytes (one full state vector,
what CPython's own seeding reads), and the reply seeds the generator exactly as
CPython would seed it from those bytes. `random.seed()` and `random.seed(None)`
make the same call. A host that answers with fixed bytes makes unseeded runs
reproducible; a host that answers a different number of bytes gets a
`RuntimeError`.

Code that seeds explicitly never calls the host. Code that does not, and runs
where nothing answers the call — a pool session without an `os=` handler, one
whose handler returns `NOT_HANDLED`, or the `monty` CLI — raises
`RuntimeError: 'os.urandom' is not supported in this environment` on its first
draw. In `pydantic_monty`, `AbstractOS.urandom()` answers from the host's
`os.urandom` by default. Under Rust's non-suspending `MontyRun::run` the draw
raises `NotImplementedError` instead, as every unanswered OS call does there.

The module-level generator is session state like the globals: a seed set in one
`feed_run` governs the next, and it survives a dump.

## Behavioural notes

- **No `SystemRandom`**, and `random.Random` cannot be subclassed (Monty has
    no class inheritance, see [classes.md](classes.md)). `random.Random.VERSION`
    on the class raises `AttributeError`; on an instance it is `3`. Instances
    have no `gauss_next` attribute.
- **Instance methods require a direct call**, as on other native objects such as `re.Pattern`.
    `rng.random()` works, but `draw = rng.random` and `getattr(rng, 'random')` raise `AttributeError`.
    Module functions can be saved and passed as callbacks: `draw = random.random` works.
- **Integer ranges are 64-bit.** `randrange`, `randint`, `choice` and friends
    raise `OverflowError: Python int too large to convert to C ssize_t` for
    bounds outside `i64`; CPython accepts any int. `getrandbits(k)` for any `k`
    and `seed(big_int)` work as in CPython.
    The sum of `sample(counts=...)` must also fit in a signed 64-bit integer.
- **Seeds.** `seed(x)` accepts `None`, `int`, `float`, `str` and `bytes`;
    there is no `bytearray`. `seed(float('nan'))` seeds from `0`, where CPython
    hashes the object's address. A `str`/`bytes` seed with a `version` other
    than `1` or `2` is hashed with Monty's own string hash where CPython's is
    randomized per process.
- **Argument coercion in the distributions** goes through a float conversion:
    a non-number raises `TypeError: must be real number, not str` where CPython
    reports the arithmetic that failed (`unsupported operand type(s) for -`).
    `binomialvariate(n, p)` requires an int `n`.
- **`sample`** accepts `list`, `tuple`, `str`, `bytes`, `range` and `deque`
    populations only (CPython accepts any `collections.abc.Sequence`); `k` and
    each `counts` entry must be ints, so `sample(x, 1.5)` raises `'float' object cannot be interpreted as an integer`
    instead of CPython's sequence-multiplication error.
- **`shuffle`** works on lists only. Any other sequence of two or more items
    raises the `does not support item assignment` error CPython's first swap
    raises, after the draw that swap would have consumed.
- **`choices`** accumulates `weights` as floats, so int weights above `2**53`
    lose precision; `cum_weights` may be any iterable of numbers.
- **`setstate`** accepts version 3 and version 2 state tuples; the third
    element (`gauss_next`) must be a float or `None`, where CPython stores any
    object.
- **Argument errors on an unseeded generator surface after the entropy call.**
    Whether a draw needs entropy is decided before its arguments are parsed, so
    `random.randint('a')` on a never-seeded generator asks the host for
    entropy and only then raises its `TypeError`; CPython raises without
    touching entropy, having seeded at import.
- **Float results match CPython on the same platform.** The distributions call
    the platform's `log`, `exp`, `sin`, `cos` and `pow`, as CPython does, so
    values can differ in the last bits between operating systems.
    `binomialvariate` uses the `libm` crate's `lgamma` where CPython carries its
    own implementation, which can move a borderline acceptance test.
    Float-power overflow messages use glibc's `(34, 'Numerical result out of range')` on every platform.
- **Arity errors do not count `self`.** `random.seed(1, 2, 3)` reports
    `takes from 0 to 2 positional arguments but 3 were given` where CPython,
    calling a bound method, says `from 1 to 3 ... but 4`. `getstate(1)` reports
    `takes no arguments (1 given)`.
- `randrange(start, None, 1)` treats an explicit `1` step as the default, as
    CPython does through small-int identity, so it succeeds; `True` is not `1`
    there and raises `Missing a non-None stop argument` in both.
