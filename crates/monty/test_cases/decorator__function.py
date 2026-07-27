# === Function decorator: identity returns the function unchanged ===
def identity(fn):
    return fn


@identity
def greet():
    return 'hi'


assert greet() == 'hi'


# === Decorator can replace the function with any value ===
def replace_with_number(fn):
    return 42


@replace_with_number
def replaced():
    return 'never runs'


assert replaced == 42


# === Wrapping: the classic decorator shape ===
def shout(fn):
    def wrapper(name):
        return fn(name).upper()

    return wrapper


@shout
def hello(name):
    return 'hello ' + name


assert hello('world') == 'HELLO WORLD'


# === Wrapper forwarding positional and keyword arguments ===
def double_result(fn):
    def wrapper(a, b=10):
        return fn(a, b) * 2

    return wrapper


@double_result
def add(a, b):
    return a + b


assert add(1) == 22
assert add(1, 2) == 6
assert add(1, b=3) == 8


# === Decorator factory (decorator with arguments) ===
def repeat(times):
    def deco(fn):
        def wrapper():
            return fn() * times

        return wrapper

    return deco


@repeat(3)
def ha():
    return 'ha'


assert ha() == 'hahaha'


# === Stacked decorators apply bottom-up (nearest the def first) ===
applied = []


def tag(label):
    def deco(fn):
        applied.append(label)
        return fn

    return deco


@tag('outer')
@tag('middle')
@tag('inner')
def stacked():
    pass


assert applied == ['inner', 'middle', 'outer']


# === Stacking composes in the right order ===
def add_one(fn):
    return lambda: fn() + 1


def times_two(fn):
    return lambda: fn() * 2


@add_one
@times_two
def base():
    return 5


# times_two applies first (5 * 2 = 10), then add_one (10 + 1 = 11)
assert base() == 11


# === Registry pattern: decorator records the function and returns it ===
# Keyed explicitly rather than by `fn.__name__` — Monty functions expose no
# introspection attributes (see limitations/language.md).
registry = {}


def register_as(key):
    def deco(fn):
        registry[key] = fn
        return fn

    return deco


@register_as('alpha')
def alpha():
    return 'a'


@register_as('beta')
def beta():
    return 'b'


assert sorted(registry) == ['alpha', 'beta']
assert registry['alpha']() == 'a'
assert registry['beta']() == 'b'


# === Decorators are arbitrary expressions, not just names ===
decos = [identity, replace_with_number]


@decos[0]
def via_subscript():
    return 'sub'


assert via_subscript() == 'sub'


# === Attribute expressions work in decorator position ===
class Holder:
    pass


Holder.deco = identity


@Holder.deco
def via_attribute():
    return 'attr'


assert via_attribute() == 'attr'


# === Decorators evaluate in the enclosing scope and capture its locals ===
def make_multiplier():
    factor = 3

    @(lambda fn: lambda: fn() * factor)
    def inner():
        return 7

    return inner()


assert make_multiplier() == 21


# === Multi-hop capture: decorator lambda reaches through two scopes ===
def outer_scope():
    offset = 100

    def middle():
        @(lambda fn: lambda: fn() + offset)
        def innermost():
            return 5

        return innermost()

    return middle()


assert outer_scope() == 105


# === Decorator factory capturing an enclosing local ===
def make_scaled():
    scale = 4

    def scaler(fn):
        def wrapper():
            return fn() * scale

        return wrapper

    @scaler
    def value():
        return 6

    return value()


assert make_scaled() == 24


# === Decorators evaluate top-down, before the function is built ===
evaluated = []


def note(label):
    evaluated.append(label)
    return identity


@note('first')
@note('second')
def ordering():
    pass


assert evaluated == ['first', 'second']


# === A raising decorator unwinds cleanly and execution continues ===
def explode(fn):
    raise RuntimeError('boom')


try:

    @explode
    def doomed():
        pass

    assert False, 'expected the decorator to raise'
except RuntimeError as exc:
    assert str(exc) == 'boom'

# execution continues normally afterwards
assert identity(greet)() == 'hi'


# === The function name stays unbound when a decorator raises ===
try:
    undecorated_name
    assert False, 'expected NameError before definition'
except NameError:
    pass

try:

    @explode
    def undecorated_name():
        pass

except RuntimeError:
    pass

try:
    undecorated_name
    assert False, 'expected the name to stay unbound'
except NameError:
    pass


# === A non-callable decorator raises TypeError ===
not_callable = 5

try:

    @not_callable
    def bad():
        pass

    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"


# === An identity decorator returns the very same function object ===
def marker():
    return 'marked'


assert identity(marker) is marker


# === Decorating a nested function binds in the enclosing function's scope ===
def with_nested():
    @identity
    def helper():
        return 'nested'

    return helper()


assert with_nested() == 'nested'


# === Decorated async functions define and bind like any other ===
# Awaiting one is covered by decorator__function_async.py, which needs `# run-async`.
@replace_with_number
async def async_replaced():
    return 'never runs'


assert async_replaced == 42


# === A decorator name shadowed by the def it decorates is an unbound local ===
# The `def` makes `shadowed` a local of `shadowing`, so the decorator lookup
# cannot fall back to the global one — it reads the local before assignment.
def shadowed(fn):
    return fn


def shadowing():
    try:

        @shadowed
        def shadowed():
            return 'inner'

        assert False, 'expected UnboundLocalError'
    except UnboundLocalError as exc:
        return str(exc)


assert shadowing() == "cannot access local variable 'shadowed' where it is not associated with a value"


# === A walrus in a decorator expression binds in the enclosing scope ===
# Decorators evaluate in the enclosing scope, so `:=` there makes a local of
# that scope — it must not fall through to the module globals.
def passthrough(v):
    def deco(fn):
        return fn

    return deco


shadowed = 'global'


def walrus_binds_locally():
    @passthrough(shadowed := 'local')
    def f():
        pass

    return shadowed


assert walrus_binds_locally() == 'local'
assert shadowed == 'global'


# The binding is a local, so it never escapes to the module namespace.
def walrus_does_not_leak():
    @passthrough(never_global := 'inner')
    def f():
        pass

    return never_global


assert walrus_does_not_leak() == 'inner'
try:
    never_global
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'never_global' is not defined"


# `global` still redirects the walrus to the module scope.
promoted = 'before'


def walrus_with_global():
    global promoted

    @passthrough(promoted := 'after')
    def f():
        pass


walrus_with_global()
assert promoted == 'after'


# A walrus in a decorator can be captured by a nested function (needs a cell).
def walrus_captured_by_closure():
    @passthrough(captured := 11)
    def f():
        pass

    def read():
        return captured

    return read()


assert walrus_captured_by_closure() == 11


# === A deep decorator stack applies in order ===
depth_order = []


def depth_tag(n):
    def deco(fn):
        depth_order.append(n)
        return fn

    return deco


@depth_tag(0)
@depth_tag(1)
@depth_tag(2)
@depth_tag(3)
@depth_tag(4)
@depth_tag(5)
@depth_tag(6)
@depth_tag(7)
@depth_tag(8)
@depth_tag(9)
@depth_tag(10)
@depth_tag(11)
@depth_tag(12)
@depth_tag(13)
@depth_tag(14)
@depth_tag(15)
@depth_tag(16)
@depth_tag(17)
@depth_tag(18)
@depth_tag(19)
def deeply_stacked():
    pass


assert depth_order == list(reversed(range(20)))
