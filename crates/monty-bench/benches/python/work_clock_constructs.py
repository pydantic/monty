# Work-clock constructs: one statement per section, each timed against `baseline`.
#
# The calibration harness runs `prelude` followed by one section at a time, so a section's cost
# beyond `baseline` is its body's cost beyond `y = i`. Every section loops `N` times over `i`.

# === prelude ===
N = 100_000


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def get(self):
        return self.x

    def add(self, a, b=1):
        return a + b


def f0():
    return 1


def f1(a):
    return a


def f3(a, b, c):
    return a


def fdefault(a, b=2, c=3):
    return a


def fstar(*args, **kwargs):
    return 1


def make_closure():
    k = 1

    def inner(a):
        return a + k

    return inner


closure = make_closure()
p = Point(1, 2)
lst = list(range(100))
small = [1, 2, 3]
tup = (1, 2, 3)
d = {'a': 1, 'b': 2, 'c': 3}
dint = {n: n for n in range(100)}
st = set(range(100))
s = 'hello world'
long_s = 'abcdefghij' * 10
b = b'hello world'
big = 1 << 200
fl = 1.5
args3 = (1, 2, 3)
words = ['abcde'] * 20
kw = {'a': 1}

# === baseline ===
for i in range(N):
    y = i

# === int_add ===
for i in range(N):
    y = i + 1

# === int_mul ===
for i in range(N):
    y = i * 3

# === int_floordiv ===
for i in range(N):
    y = i // 3

# === int_mod ===
for i in range(N):
    y = i % 7

# === int_pow ===
for i in range(N):
    y = i**2

# === float_mul ===
for i in range(N):
    y = fl * 2.5

# === int_compare ===
for i in range(N):
    y = i < 50

# === bool_not ===
for i in range(N):
    y = not i

# === if_else ===
for i in range(N):
    if i & 1:
        y = 1
    else:
        y = 2

# === global_load ===
for i in range(N):
    y = fl

# === big_int_add ===
for i in range(N):
    y = big + i

# === big_int_mul ===
for i in range(N):
    y = big * i

# === attr_load ===
for i in range(N):
    y = p.x

# === attr_store ===
for i in range(N):
    p.x = i

# === bound_method ===
for i in range(N):
    y = p.get

# === method_call ===
for i in range(N):
    y = p.get()

# === method_call_args ===
for i in range(N):
    y = p.add(i, 2)

# === method_call_kwargs ===
for i in range(N):
    y = p.add(i, b=2)

# === instantiate ===
for i in range(N):
    y = Point(i, 2)

# === call_0 ===
for i in range(N):
    y = f0()

# === call_1 ===
for i in range(N):
    y = f1(i)

# === call_3 ===
for i in range(N):
    y = f3(i, 2, 3)

# === call_defaults ===
for i in range(N):
    y = fdefault(i)

# === call_kwargs ===
for i in range(N):
    y = f3(i, b=2, c=3)

# === call_star_args ===
for i in range(N):
    y = f3(*args3)

# === call_star_kwargs ===
for i in range(N):
    y = fstar(i, **kw)

# === call_closure ===
for i in range(N):
    y = closure(i)

# === make_function ===
for i in range(N):

    def y():
        return 1


# === make_lambda_closure ===
def make_closures():
    for i in range(N):
        y = lambda: i  # noqa: E731, F841


make_closures()

# === builtin_len ===
for i in range(N):
    y = len(lst)

# === builtin_abs ===
for i in range(N):
    y = abs(i)

# === builtin_isinstance ===
for i in range(N):
    y = isinstance(i, int)

# === builtin_min ===
for i in range(N):
    y = min(i, 5)

# === type_int ===
for i in range(N):
    y = int(fl)

# === type_str ===
for i in range(N):
    y = str(i)

# === list_subscr ===
for i in range(N):
    y = lst[5]

# === list_store ===
for i in range(N):
    lst[5] = i

# === list_slice ===
for i in range(N):
    y = lst[10:20]

# === tuple_subscr ===
for i in range(N):
    y = tup[1]

# === dict_subscr ===
for i in range(N):
    y = d['b']

# === dict_subscr_int ===
for i in range(N):
    y = dint[i % 100]

# === dict_store ===
for i in range(N):
    d['b'] = i

# === dict_get ===
for i in range(N):
    y = d.get('b')

# === str_subscr ===
for i in range(N):
    y = s[3]

# === str_slice ===
for i in range(N):
    y = long_s[10:60]

# === build_list_3 ===
for i in range(N):
    y = [i, 1, 2]

# === build_tuple_3 ===
for i in range(N):
    y = (i, 1, 2)

# === build_dict_2 ===
for i in range(N):
    y = {'a': i, 'b': 2}

# === build_set_3 ===
for i in range(N):
    y = {i, 1, 2}

# === list_append ===
out = []
for i in range(N):
    out.append(i)

# === list_pop ===
for i in range(N):
    small.append(i)
    y = small.pop()

# === list_copy_100 ===
for i in range(N):
    y = lst.copy()

# === list_concat_100 ===
for i in range(N):
    y = lst + small

# === list_repeat_100 ===
for i in range(N):
    y = small * 33

# === unpack_tuple ===
for i in range(N):
    y, z, w = tup

# === unpack_star ===
for i in range(N):
    y, *z = tup

# === in_list_100_miss ===
for i in range(N):
    y = -1 in lst

# === in_list_3 ===
for i in range(N):
    y = 3 in small

# === in_dict ===
for i in range(N):
    y = 'b' in d

# === in_set ===
for i in range(N):
    y = 50 in st

# === in_str ===
for i in range(N):
    y = 'wor' in s

# === list_eq_100 ===
other = list(lst)
for i in range(N):
    y = lst == other

# === str_eq_100 ===
other = 'abcdefghij' * 10
for i in range(N):
    y = long_s == other

# === str_concat ===
for i in range(N):
    y = s + s

# === str_concat_100 ===
for i in range(N):
    y = long_s + long_s

# === str_repeat ===
for i in range(N):
    y = s * 10

# === fstring ===
for i in range(N):
    y = f'{i} and {s}'

# === fstring_spec ===
for i in range(N):
    y = f'{fl:.2f}'

# === str_format_percent ===
for i in range(N):
    y = '%d-%s' % (i, s)

# === str_upper ===
for i in range(N):
    y = s.upper()

# === str_split ===
for i in range(N):
    y = s.split()

# === str_join ===
parts = ['a', 'b', 'c']
for i in range(N):
    y = ','.join(parts)

# === str_find_100 ===
for i in range(N):
    y = long_s.find('zz')

# === str_replace_100 ===
for i in range(N):
    y = long_s.replace('a', 'bb')

# === str_startswith ===
for i in range(N):
    y = s.startswith('hello')

# === bytes_subscr ===
for i in range(N):
    y = b[3]

# === bytes_concat ===
for i in range(N):
    y = b + b

# === for_range_10 ===
for i in range(N):
    for y in range(10):
        pass

# === for_list_3 ===
for i in range(N):
    for y in small:
        pass

# === while_10 ===
for i in range(N):
    y = 0
    while y < 10:
        y += 1

# === list_comp_10 ===
for i in range(N):
    y = [n for n in range(10)]

# === dict_comp_10 ===
for i in range(N):
    y = {n: n for n in range(10)}

# === sum_list_100 ===
for i in range(N):
    y = sum(lst)

# === sorted_3 ===
for i in range(N):
    y = sorted(small)

# === sorted_100 ===
for i in range(N):
    y = sorted(lst)

# === enumerate_3 ===
for i in range(N):
    for y in enumerate(small):
        pass

# === zip_3 ===
for i in range(N):
    for y in zip(small, tup):
        pass

# === try_no_raise ===
for i in range(N):
    try:
        y = i
    except ValueError:
        y = 0

# === try_raise ===
for i in range(N):
    try:
        raise ValueError
    except ValueError:
        y = 0

# === try_raise_message ===
for i in range(N):
    try:
        raise ValueError('bad value')
    except ValueError as e:
        y = e

# === assert ===
for i in range(N):
    assert i >= 0

# === ternary ===
for i in range(N):
    y = 1 if i else 2

# === chained_compare ===
for i in range(N):
    y = 0 <= i < 50

# === and_or ===
for i in range(N):
    y = i and fl or 3

# === const_str ===
for i in range(N):
    y = 'hello'

# === const_float ===
for i in range(N):
    y = 2.5

# === const_tuple ===
for i in range(N):
    y = (1, 2, 3)

# === const_big_int ===
for i in range(N):
    y = 123456789012345678901234567890

# === fstring_1 ===
for i in range(N):
    y = f'{i}'

# === fstring_str_2 ===
for i in range(N):
    y = f'{s}-{s}'

# === fstring_repr ===
for i in range(N):
    y = f'{s!r}'

# === str_of_str ===
for i in range(N):
    y = str(s)

# === repr_str ===
for i in range(N):
    y = repr(s)

# === list_slice_3 ===
for i in range(N):
    y = small[1:]

# === list_slice_step ===
for i in range(N):
    y = lst[::2]

# === str_slice_3 ===
for i in range(N):
    y = s[1:4]

# === tuple_slice ===
for i in range(N):
    y = tup[1:]

# === str_strip ===
for i in range(N):
    y = s.strip()

# === str_lower ===
for i in range(N):
    y = long_s.lower()

# === str_count_100 ===
for i in range(N):
    y = long_s.count('a')

# === str_split_100 ===
for i in range(N):
    y = long_s.split('a')

# === str_join_100 ===
for i in range(N):
    y = ''.join(words)

# === str_encode ===
for i in range(N):
    y = s.encode()

# === str_isdigit ===
for i in range(N):
    y = s.isdigit()

# === str_format_method ===
for i in range(N):
    y = '{} and {}'.format(i, s)

# === dict_items_3 ===
for i in range(N):
    for y in d.items():
        pass

# === dict_keys_list ===
for i in range(N):
    y = list(d)

# === list_index ===
for i in range(N):
    y = lst.index(50)

# === list_reverse_3 ===
for i in range(N):
    small.reverse()

# === list_of_range_10 ===
for i in range(N):
    y = list(range(10))

# === tuple_of_list ===
for i in range(N):
    y = tuple(small)

# === set_of_list ===
for i in range(N):
    y = set(small)

# === len_str ===
for i in range(N):
    y = len(s)

# === exception_new ===
for i in range(N):
    y = ValueError('bad value')

# === range_new ===
for i in range(N):
    y = range(10)

# === any_3 ===
for i in range(N):
    y = any(small)

# === max_list_100 ===
for i in range(N):
    y = max(lst)

# === int_of_str ===
for i in range(N):
    y = int('12345')

# === float_of_str ===
for i in range(N):
    y = float('1.5')

# === round_float ===
for i in range(N):
    y = round(fl, 1)

# === isinstance_tuple ===
for i in range(N):
    y = isinstance(s, (int, str))

# === assert_eq_str ===
for i in range(N):
    assert s == 'hello world'

# === assert_eq_list ===
for i in range(N):
    assert small == [1, 2, 3]

# The big-integer sections run the baseline loop, then their own shorter one: their operations
# are too slow for `N` iterations, so read their ratio, not their units per iteration.

# === big_mul_30_limbs ===
a = (1 << 1920) - 1
c = (1 << 1900) - 3
for i in range(N):
    y = i
for j in range(20_000):
    z = a * c

# === big_mul_300_limbs ===
a = (1 << 19200) - 1
c = (1 << 19000) - 3
for i in range(N):
    y = i
for j in range(1000):
    z = a * c

# === big_mul_3000_limbs ===
a = (1 << 192000) - 1
c = (1 << 190000) - 3
for i in range(N):
    y = i
for j in range(30):
    z = a * c

# === big_mul_3000_by_1 ===
a = (1 << 192000) - 1
for i in range(N):
    y = i
for j in range(5000):
    z = a * 12345

# === big_div_300_limbs ===
a = 3**24000
c = 7**6800
for i in range(N):
    y = i
for j in range(500):
    z = a // c

# === big_pow_500_limbs ===
for i in range(N):
    y = i
for j in range(200):
    z = 3**20000

# === big_pow_mod_30_limbs ===
a = (1 << 1920) - 1
c = (1 << 1900) - 3
for i in range(N):
    y = i
for j in range(200):
    z = pow(c, 65537, a)

# === big_str_200_limbs ===
a = (1 << 13000) - 1
for i in range(N):
    y = i
for j in range(1000):
    z = str(a)

# === big_str_30_limbs ===
a = (1 << 1920) - 1
for i in range(N):
    y = i
for j in range(20_000):
    z = str(a)

# === big_from_str_200_limbs ===
t = str((1 << 13000) - 1)
for i in range(N):
    y = i
for j in range(1000):
    z = int(t)

# === big_add_300_limbs ===
a = (1 << 19200) - 1
c = (1 << 19000) - 3
for i in range(N):
    y = i
for j in range(20_000):
    z = a + c

# === big_pow_mod_2000_limbs ===
a = (1 << 131072) + 1
for i in range(N):
    y = i
for j in range(2):
    z = pow(3, (1 << 33) - 1, a)

# === big_pow_1500_limbs ===
for i in range(N):
    y = i
for j in range(200):
    z = 2**100000
