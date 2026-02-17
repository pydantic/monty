# === Basic **kwargs unpacking ===
def greet(name, greeting):
    return f'{greeting}, {name}!'


opts = {'greeting': 'Hi'}
assert greet('Alice', **opts) == 'Hi, Alice!', 'basic **kwargs unpacking'

# === Dict literal unpacking ===
assert greet('Charlie', **{'greeting': 'Hey'}) == 'Hey, Charlie!', 'dict literal unpacking'


# === Multiple kwargs in unpacked dict ===
def format_msg(msg, prefix, suffix):
    return f'{prefix}{msg}{suffix}'


assert format_msg('test', **{'prefix': '[', 'suffix': ']'}) == '[test]', 'multiple kwargs unpacking'

# === Combining regular kwargs with **kwargs ===
assert format_msg('hello', prefix='> ', **{'suffix': '!'}) == '> hello!', 'regular kwargs with **kwargs'


# === **kwargs with positional args ===
def add_all(a, b, c):
    return a + b + c


assert add_all(1, 2, **{'c': 3}) == 6, '**kwargs with positional args'
assert add_all(1, **{'b': 2, 'c': 3}) == 6, '**kwargs providing multiple args'

# === Variable dict unpacking ===
settings = {'prefix': '>>> ', 'suffix': ' <<<'}
assert format_msg('output', **settings) == '>>> output <<<', 'variable dict unpacking'


# === Unpacking with keyword-only args ===
def kwonly_func(a, *, b, c):
    return a + b + c


assert kwonly_func(1, **{'b': 2, 'c': 3}) == 6, '**kwargs with keyword-only args'


# === Empty dict unpacking with all args provided ===
def simple(x, y):
    return x + y


assert simple(1, 2, **{}) == 3, 'empty dict unpacking'


# === All kwargs from unpacking ===
def all_kwargs(a, b, c):
    return a * 100 + b * 10 + c


assert all_kwargs(**{'a': 1, 'b': 2, 'c': 3}) == 123, 'all args from **kwargs'
assert all_kwargs(**{'c': 7, 'a': 4, 'b': 5}) == 457, 'all args from **kwargs different order'


# === Dynamic **kwargs keys ===
def kwonly_echo(*, keyword):
    return keyword


key_name = 'k' + 'e' + 'y' + 'w' + 'o' + 'r' + 'd'
assert kwonly_echo(**{key_name: 'dynamic'}) == 'dynamic', 'runtime string key matches kw-only param'


# ============================================================
# *args unpacking tests (function calls)
# ============================================================


# === *args with zero args ===
def no_args():
    return 'ok'


assert no_args(*[]) == 'ok', '*args with empty list'
assert no_args(*()) == 'ok', '*args with empty tuple'


# === *args with one arg ===
def one_arg(x):
    return x * 2


assert one_arg(*[5]) == 10, '*args with one item list'
assert one_arg(*(7,)) == 14, '*args with one item tuple'


# === *args with two args ===
def two_args(a, b):
    return a + b


assert two_args(*[1, 2]) == 3, '*args with two item list'
assert two_args(*(3, 4)) == 7, '*args with two item tuple'


# === *args with three+ args ===
def many_args(a, b, c, d):
    return a + b + c + d


assert many_args(*[1, 2, 3, 4]) == 10, '*args with four items'
assert many_args(*(10, 20, 30, 40)) == 100, '*args with tuple four items'


# === Mixed positional and *args ===
assert two_args(1, *[2]) == 3, 'pos + *args'
assert many_args(1, 2, *[3, 4]) == 10, 'two pos + *args'


# === *args with heap-allocated values ===
def list_arg(lst):
    return len(lst)


my_list = [1, 2, 3]
assert list_arg(*[my_list]) == 3, '*args with list value'


# ============================================================
# Combined *args and **kwargs (function calls)
# ============================================================


# === *args and **kwargs together ===
def mixed_func(a, b, c):
    return f'{a}-{b}-{c}'


assert mixed_func(*[1], **{'b': 2, 'c': 3}) == '1-2-3', '*args and **kwargs'
assert mixed_func(*[1, 2], **{'c': 3}) == '1-2-3', 'two *args and **kwargs'


# === *args tuple with **kwargs ===
args_tuple = (10, 20)
kwargs_dict = {'c': 30}
assert many_args(*args_tuple, **kwargs_dict, d=40) == 100, '*args tuple + **kwargs + regular kwarg'


# === Empty *args with **kwargs ===
assert mixed_func(*[], **{'a': 'x', 'b': 'y', 'c': 'z'}) == 'x-y-z', 'empty *args with **kwargs'


# === *args with empty **kwargs ===
assert two_args(*[5, 6], **{}) == 11, '*args with empty **kwargs'


# === All combinations: pos, *args, kwargs, **kwargs ===
def full_func(a, b, c, d):
    return a * 1000 + b * 100 + c * 10 + d


assert full_func(1, *[2], c=3, **{'d': 4}) == 1234, 'pos + *args + kwarg + **kwargs'


# === *args with heap values and **kwargs ===
def heap_func(lst, dct):
    return len(lst) + len(dct)


list_val = [1, 2, 3]
dict_val = {'a': 1}
assert heap_func(*[list_val], **{'dct': dict_val}) == 4, '*args and **kwargs with heap values'


# === Both *args and **kwargs empty ===
assert no_args(*[], **{}) == 'ok', 'empty *args and empty **kwargs'
