# `random` module

Monty's `random` uses CPython's MT19937 core, ported from `Modules/_randommodule.c`, with the method bodies of
`Lib/random.py` ported on top.
A seeded generator produces CPython 3.14's sequence: `random.seed(42)` gives the same `random()`, `randint()`,
`choice()`, `shuffle()`, `sample()` and `choices()` results as CPython, and the same distribution values on the same
platform (see the note on floats below).

## Implemented

- Module functions: `random`, `seed`, `getstate`, `setstate`, `getrandbits`, `randbytes`, `randrange`, `randint`,
    `choice`, `choices`, `shuffle`, `sample`, `uniform`, `triangular`, `normalvariate`, `gauss`, `lognormvariate`,
    `expovariate`, `vonmisesvariate`, `gammavariate`, `betavariate`, `paretovariate`, `weibullvariate`,
    `binomialvariate`.
- `random.Random(x=None)` instances with the same methods and an independent state, plus `instance.VERSION`.

## Entropy

An unseeded generator seeds itself from the host on its first draw.
The draw suspends with an `os.urandom` host call for 2496 bytes, the 624 32-bit words of one MT19937 state vector,
and the reply seeds the generator as CPython's `seed(None)` does from the same bytes.
This applies to the module-level generator and to a `random.Random()` created without a seed.
`random.seed()` and `random.seed(None)` make the same call.
A host that answers with fixed bytes makes unseeded runs reproducible.
A reply of any other length, or one that is not `bytes`, raises `RuntimeError`.

Code that seeds explicitly never calls the host.
Where nothing answers the call, the first unseeded draw raises
`RuntimeError: 'os.urandom' is not supported in this environment`.
That is the case in a pool session without an `os=` handler, in one whose handler returns `NOT_HANDLED`, and in the
`monty` CLI with a `--mount`.
In `pydantic_monty`, `AbstractOS.urandom()` returns the host's `os.urandom(size)` by default.
Under Rust's non-suspending `MontyRun::run`, which the CLI uses without a mount, the draw raises
`NotImplementedError`, as every unanswered OS call does there.
`getstate()` on a never-seeded generator makes the same call first, since there is no state to report until then.

The module-level generator is session state like the globals: a seed set in one `feed_run` applies to the next, and
it is included in a dump.

## Behavioural notes

- **`Random` instances do not convert to host values.** Returning a `Random` instance, `random.Random` or `type(rng)`
    to the host produces `MontyValue::Repr` in Rust and a string in Python.
    Return the generated values or `rng.getstate()` instead.
- **No `SystemRandom`**, and `random.Random` cannot be subclassed (Monty has no class inheritance, see
    [classes.md](classes.md)).
    `random.Random.VERSION` on the class raises `AttributeError`; on an instance it is `3`.
    Instances have no `gauss_next` attribute.
- **Copying an unseeded generator gives two independent streams.** `copy.copy(rng)` and `copy.deepcopy(rng)` rebuild
    a generator at the same point in the same sequence, but one that has never been seeded has no state to carry, so
    each copy takes its own entropy from the host on its first draw.
    CPython seeds at construction, so its copies agree.
    See [copy.md](copy.md).
- **Instance methods must be called directly**, as on other native objects such as `re.Pattern`.
    `rng.random()` works, but `draw = rng.random` and `getattr(rng, 'random')` raise `AttributeError`.
    Module functions can be stored and passed as callbacks: `draw = random.random` works.
- **Integer ranges are 64-bit.** `randrange`, `randint`, `choice` and `sample` raise
    `OverflowError: Python int too large to convert to C ssize_t` for bounds outside `i64`; CPython accepts any int.
    `seed(big_int)` accepts any int, as in CPython.
    `getrandbits(k)` raises `OverflowError: Python int too large for C uint64_t` from `k >= 2**63`, where CPython
    accepts up to `2**64` and then fails to allocate; `randbytes(n)` raises it from `n >= 2**61` in both.
    The sum of `sample(counts=...)` must also fit in a signed 64-bit integer.
- **Seeds.** `seed(x)` accepts `None`, `int`, `float`, `str` and `bytes`; there is no `bytearray`.
    `seed(float('nan'))` seeds from `0`, where CPython hashes the object's address.
    A `str`/`bytes` seed with a `version` other than `1` or `2` is hashed with Monty's own string hash, where CPython's
    hash is randomized per process.
- **The distributions convert their arguments to float**, so a non-number raises `TypeError: must be real number, not str` where CPython reports the arithmetic that failed (`unsupported operand type(s) for -`).
    `binomialvariate(n, p)` requires an int `n`.
- **`sample`** accepts `list`, `tuple`, `str`, `bytes`, `range` and `deque` populations only (CPython accepts any
    `collections.abc.Sequence`).
    `k` and each `counts` entry must be ints, so `sample(x, 1.5)` raises
    `'float' object cannot be interpreted as an integer` instead of CPython's sequence-multiplication error.
- **`choices`** accumulates `weights` as floats, so int weights above `2**53` lose precision; `cum_weights` may be
    any iterable of numbers.
- **`setstate`** accepts version 3 and version 2 state tuples; the third element (`gauss_next`) must be `None` or
    a `float`, `int` or `bool`, and is stored as a float, where CPython stores any object.
    A state word in `2**63..2**64` is truncated to 32 bits as on 64-bit CPython; CPython on Windows raises
    `OverflowError` for it.
- **Argument errors on an unseeded generator are raised after the entropy call.** Whether a draw needs entropy is
    decided before its arguments are parsed, so `random.randint('a')` on a never-seeded generator requests entropy
    from the host and only then raises its `TypeError`.
    CPython seeds at import, so it raises without reading entropy.
- **Float results match CPython on the same platform.** The distributions call the platform's `log`, `exp`, `sin`,
    `cos` and `pow`, as CPython does, so values can differ in the last bits between operating systems.
    `binomialvariate` uses the `libm` crate's `lgamma` where CPython has its own implementation, which can change the
    outcome of a borderline acceptance test.
    Float-power overflow messages use glibc's `(34, 'Numerical result out of range')` on every platform.
- **Arity errors do not count `self`.** `random.seed(1, 2, 3)` reports
    `takes from 0 to 2 positional arguments but 3 were given` where CPython, calling a bound method, says
    `from 1 to 3 ... but 4`.
    `getstate(1)` reports `takes no arguments (1 given)`.
- `randrange(start, None, 1)` treats an explicit `1` step as the default, as CPython does through small-int identity,
    so it succeeds; `True` is not `1` there and raises `Missing a non-None stop argument` in both.
