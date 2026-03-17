def f(*args, **kwargs):
    return args, kwargs


# === Multiple *args ===
assert f(*[1, 2], *[3, 4]) == ((1, 2, 3, 4), {}), 'multiple star args'
assert f(0, *[1, 2], 3) == ((0, 1, 2, 3), {}), 'positional after star args'
assert f(*[], *[1]) == ((1,), {}), 'unpack empty then non-empty'

# === Multiple **kwargs ===
assert f(**{'a': 1}, **{'b': 2}) == ((), {'a': 1, 'b': 2}), 'multiple star-star kwargs'
assert f(**{'a': 1}, b=2) == ((), {'a': 1, 'b': 2}), 'named after star-star'
assert f(key='before', **{'a': 1}) == ((), {'key': 'before', 'a': 1}), 'named before star-star'

# === Mixed ===
assert f(1, *[2, 3], **{'x': 4}) == ((1, 2, 3), {'x': 4}), 'mixed star and star-star'

# === Builtin callable with GeneralizedCall (Callable::Builtin path) ===
# max(*[1,2], *[3,4]) exercises the Callable::Builtin branch in compile_call GeneralizedCall
result = max(*[1, 2], *[3, 4])
assert result == 4, 'builtin max with multiple *args'

result = min(*[5, 3], *[7, 1])
assert result == 1, 'builtin min with multiple *args'

# Builtin type and exception constructors should keep their public names in
# **kwargs merge errors, not fall back to '<unknown>'.
try:
    list(**1)
    assert False, 'list with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('list() argument after ** must be a mapping, not int',), (
        'builtin type non-mapping **kwargs error keeps type name'
    )

try:
    ValueError(**1)
    assert False, 'ValueError with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('ValueError() argument after ** must be a mapping, not int',), (
        'builtin exception non-mapping **kwargs error keeps exception name'
    )

try:
    list(a=1, **{'a': 2})
    assert False, 'list with duplicate **kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ("list() got multiple values for keyword argument 'a'",), (
        'builtin type duplicate **kwargs error keeps type name'
    )

# Builtin type constructors should also keep their public names in
# non-mapping **kwargs errors so compiler call metadata matches CPython.
try:
    bool(**1)
    assert False, 'bool with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('bool() argument after ** must be a mapping, not int',), (
        'bool non-mapping **kwargs error keeps builtin type name'
    )

try:
    int(**1)
    assert False, 'int with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('int() argument after ** must be a mapping, not int',), (
        'int non-mapping **kwargs error keeps builtin type name'
    )

try:
    float(**1)
    assert False, 'float with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('float() argument after ** must be a mapping, not int',), (
        'float non-mapping **kwargs error keeps builtin type name'
    )

try:
    str(**1)
    assert False, 'str with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('str() argument after ** must be a mapping, not int',), (
        'str non-mapping **kwargs error keeps builtin type name'
    )

try:
    bytes(**1)
    assert False, 'bytes with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('bytes() argument after ** must be a mapping, not int',), (
        'bytes non-mapping **kwargs error keeps builtin type name'
    )

try:
    tuple(**1)
    assert False, 'tuple with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('tuple() argument after ** must be a mapping, not int',), (
        'tuple non-mapping **kwargs error keeps builtin type name'
    )

try:
    dict(**1)
    assert False, 'dict with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('dict() argument after ** must be a mapping, not int',), (
        'dict non-mapping **kwargs error keeps builtin type name'
    )

try:
    set(**1)
    assert False, 'set with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('set() argument after ** must be a mapping, not int',), (
        'set non-mapping **kwargs error keeps builtin type name'
    )

try:
    frozenset(**1)
    assert False, 'frozenset with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('frozenset() argument after ** must be a mapping, not int',), (
        'frozenset non-mapping **kwargs error keeps builtin type name'
    )

try:
    range(**1)
    assert False, 'range with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('range() argument after ** must be a mapping, not int',), (
        'range non-mapping **kwargs error keeps builtin type name'
    )

try:
    slice(**1)
    assert False, 'slice with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('slice() argument after ** must be a mapping, not int',), (
        'slice non-mapping **kwargs error keeps builtin type name'
    )

try:
    type(**1)
    assert False, 'type with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('type() argument after ** must be a mapping, not int',), (
        'type non-mapping **kwargs error keeps builtin type name'
    )

try:
    property(**1)
    assert False, 'property with non-mapping **arg should raise TypeError'
except TypeError as e:
    assert e.args == ('property() argument after ** must be a mapping, not int',), (
        'property non-mapping **kwargs error keeps builtin type name'
    )

try:
    ValueError(a=1, **{'a': 2})
    assert False, 'ValueError with duplicate **kwargs should raise TypeError'
except TypeError as e:
    assert e.args == ("ValueError() got multiple values for keyword argument 'a'",), (
        'builtin exception duplicate **kwargs error keeps exception name'
    )

# === Expression-based callable with GeneralizedCall (compile_call_args path) ===
# funcs[0](*[1,2], *[3,4]) exercises the GeneralizedCall branch in compile_call_args
funcs = [f]
result = funcs[0](*[1, 2], *[3, 4])
assert result == ((1, 2, 3, 4), {}), 'subscript call with multiple *args'

result = funcs[0](**{'a': 1}, **{'b': 2})
assert result == ((), {'a': 1, 'b': 2}), 'subscript call with multiple **kwargs'

# === Named kwarg in GeneralizedCall (compile_generalized_call_body Named path) ===
# f(*[1,2], *[3], x=5): two *unpacks → GeneralizedCall; x=5 is a Named kwarg.
# This exercises the CallKwarg::Named arm in compile_generalized_call_body.
result = f(*[1, 2], *[3], x=5)
assert result == ((1, 2, 3), {'x': 5}), 'named kwarg in multi-star GeneralizedCall'

result = funcs[0](*[1, 2], *[3], x=5)
assert result == ((1, 2, 3), {'x': 5}), 'subscript call: named kwarg in GeneralizedCall'
