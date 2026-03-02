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
