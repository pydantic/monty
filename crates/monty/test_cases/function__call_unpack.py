def f(*args, **kwargs):
    return args, kwargs


# === Direct list and tuple unpacking ===
def exact(a, b, c=3):
    return a, b, c


left = [1, 2]
pair = (1, 2)
assert exact(*left) == exact(*pair) == (1, 2, 3)
assert exact(0, *pair) == (0, 1, 2)
assert exact(*pair, c=4) == (1, 2, 4)
assert exact(*range(3)) == (0, 1, 2)
assert len(f(*([1] * 256))[0]) == 256


def replace_left():
    left[0] = 10
    return (4,)


assert exact(*left, *replace_left()) == (1, 2, 4)
assert exact(*left) == (10, 2, 3)


# === Direct dict unpacking ===
assert exact(**{'b': 2, 'a': 1}) == (1, 2, 3)
assert exact(1, **{'c': 4, 'b': 2}) == (1, 2, 4)
assert exact(*(1,), **{'c': 4, 'b': 2}) == (1, 2, 4)
assert exact(1, 2, **{}) == (1, 2, 3)
assert (lambda: 42)(**{}) == 42


def named(long_keyword_parameter):
    return long_keyword_parameter


prefix = 'long_keyword_'
key = prefix + 'parameter'
argument = []
assert named(**{key: argument}) is argument

sparse = {'unused': 0, 'a': 1, 'b': 2}
sparse.pop('unused')
assert exact(**sparse) == (1, 2, 3)

try:
    exact(**{'unknown': [], 1: []})
    assert False, 'expected TypeError for a non-string keyword'
except TypeError as e:
    assert e.args == ('keywords must be strings',)

source = {'a': 1}
captured = f(**source)[1]
captured['a'] = 2
assert source == {'a': 1}


# === Named arguments mixed with unpacking and variadic capture ===
def combined(head, *rest, flag=False, **extras):
    return head, rest, flag, extras


sequence = [2, 3]
mapping = {'key': 4}


def set_flag():
    sequence[0] = 20
    mapping['key'] = 40
    return True


assert combined(1, *sequence, flag=set_flag(), **mapping) == (1, (2, 3), True, {'key': 40})
assert sequence == [20, 3]
assert combined(1, *sequence, flag=True, **{}) == (1, (20, 3), True, {})
captured = combined(1, *sequence, first=10, flag=True, **mapping)[3]
assert list(captured) == ['first', 'key']
captured['key'] = 50
assert mapping == {'key': 40}


def fixed(head, next_value, tail, flag):
    return head, next_value, tail, flag


assert fixed(1, *[2], flag=True, **{'tail': 3}) == (1, 2, 3, True)

try:
    combined(1, *sequence, flag=True, **{'flag': False})
    assert False, 'expected TypeError for a repeated keyword'
except TypeError as e:
    assert e.args == ("combined() got multiple values for keyword argument 'flag'",)

try:
    combined(1, *sequence, extra=[], **{'extra': []})
    assert False, 'expected TypeError for a repeated captured keyword'
except TypeError as e:
    assert e.args == ("combined() got multiple values for keyword argument 'extra'",)

try:
    combined(1, *sequence, **{'head': []})
    assert False, 'expected TypeError for a keyword duplicating a positional argument'
except TypeError as e:
    assert e.args == ("combined() got multiple values for argument 'head'",)


def with_default_slots(a=1, b=2, /, c=3, *rest, enabled=False, needed, payload=[], **unused):
    return a, b, c, rest, enabled, needed, payload, unused


result = with_default_slots(8, *(), **{'z': 40, 'needed': True, 'c': 30, 'q': 50})
assert result == (8, 2, 30, (), False, True, [], {'z': 40, 'q': 50})
assert list(result[-1]) == ['z', 'q']

try:
    with_default_slots(**{'unknown': []})
    assert False, 'expected TypeError for a missing keyword-only parameter'
except TypeError as e:
    assert e.args == ("with_default_slots() missing 1 required keyword-only argument: 'needed'",)

supplied = (1, 2)
capture_args, capture_kwargs = f(*supplied, **mapping)
assert capture_args == supplied
assert capture_args is not supplied
capture_kwargs['key'] = 50
assert mapping == {'key': 40}
assert f(1, 2, z=3, a=4) == ((1, 2), {'z': 3, 'a': 4})
assert list(f(1, z=3, a=4)[1]) == ['z', 'a']
assert f() == ((), {})


# === Multiple *args ===
assert f(*[1, 2], *[3, 4]) == ((1, 2, 3, 4), {})
assert f(0, *[1, 2], 3) == ((0, 1, 2, 3), {})
assert f(*[], *[1]) == ((1,), {})

# === Multiple **kwargs ===
assert f(**{'a': 1}, **{'b': 2}) == ((), {'a': 1, 'b': 2})
assert f(**{'a': 1}, b=2) == ((), {'a': 1, 'b': 2})
assert f(key='before', **{'a': 1}) == ((), {'key': 'before', 'a': 1})

# === Mixed ===
assert f(1, *[2, 3], **{'x': 4}) == ((1, 2, 3), {'x': 4})

# === Builtin callable with GeneralizedCall (Callable::Builtin path) ===
# max(*[1,2], *[3,4]) exercises the Callable::Builtin branch in compile_call GeneralizedCall
result = max(*[1, 2], *[3, 4])
assert result == 4

result = min(*[5, 3], *[7, 1])
assert result == 1

# Builtin type and exception constructors should keep their public names in
# **kwargs merge errors, not fall back to '<unknown>'.
try:
    list(**1)
    assert False, 'list with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('list() argument after ** must be a mapping, not int',)

try:
    ValueError(**1)
    assert False, 'ValueError with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('ValueError() argument after ** must be a mapping, not int',)

try:
    list(a=1, **{'a': 2})
    assert False, 'list with duplicate **kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ("list() got multiple values for keyword argument 'a'",)

# Builtin type constructors should also keep their public names in
# non-mapping **kwargs errors so compiler call metadata matches CPython.
try:
    bool(**1)
    assert False, 'bool with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('bool() argument after ** must be a mapping, not int',)

try:
    int(**1)
    assert False, 'int with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('int() argument after ** must be a mapping, not int',)

try:
    float(**1)
    assert False, 'float with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('float() argument after ** must be a mapping, not int',)

try:
    str(**1)
    assert False, 'str with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('str() argument after ** must be a mapping, not int',)

try:
    bytes(**1)
    assert False, 'bytes with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('bytes() argument after ** must be a mapping, not int',)

try:
    tuple(**1)
    assert False, 'tuple with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('tuple() argument after ** must be a mapping, not int',)

try:
    dict(**1)
    assert False, 'dict with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('dict() argument after ** must be a mapping, not int',)

try:
    set(**1)
    assert False, 'set with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('set() argument after ** must be a mapping, not int',)

try:
    frozenset(**1)
    assert False, 'frozenset with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('frozenset() argument after ** must be a mapping, not int',)

try:
    range(**1)
    assert False, 'range with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('range() argument after ** must be a mapping, not int',)

try:
    slice(**1)
    assert False, 'slice with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('slice() argument after ** must be a mapping, not int',)

try:
    type(**1)
    assert False, 'type with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('type() argument after ** must be a mapping, not int',)

try:
    property(**1)
    assert False, 'property with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('property() argument after ** must be a mapping, not int',)

try:
    ValueError(a=1, **{'a': 2})
    assert False, 'ValueError with duplicate **kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ("ValueError() got multiple values for keyword argument 'a'",)

# === Expression-based callable with GeneralizedCall (compile_call_args path) ===
# funcs[0](*[1,2], *[3,4]) exercises the GeneralizedCall branch in compile_call_args
funcs = [f]
result = funcs[0](*[1, 2], *[3, 4])
assert result == ((1, 2, 3, 4), {})

result = funcs[0](**{'a': 1}, **{'b': 2})
assert result == ((), {'a': 1, 'b': 2})

# === Named kwarg in GeneralizedCall (compile_generalized_call_body Named path) ===
# f(*[1,2], *[3], x=5): two *unpacks → GeneralizedCall; x=5 is a Named kwarg.
# This exercises the CallKwarg::Named arm in compile_generalized_call_body.
result = f(*[1, 2], *[3], x=5)
assert result == ((1, 2, 3), {'x': 5})

result = funcs[0](*[1, 2], *[3], x=5)
assert result == ((1, 2, 3), {'x': 5})
