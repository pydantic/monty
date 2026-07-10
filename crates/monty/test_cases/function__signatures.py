# === Basic functions ===
def simple(a, b, c):
    return a + b + c


assert simple(1, 2, 3) == 6
assert simple(10, 20, 30) == 60


# === Positional-only parameters ===
def pos_only(a, b, /, c):
    return a + b + c


assert pos_only(1, 2, 3) == 6
assert pos_only(5, 5, 5) == 15
assert pos_only(5, 5, c=5) == 15


# === All positional-only ===
def all_pos_only(a, b, c, /):
    return a + b + c


assert all_pos_only(1, 2, 3) == 6


# === Multiple parameter groups ===
def multi_group(a, /, b, c):
    return f'a={a} b={b} c={c}'


assert multi_group(1, 2, 3) == 'a=1 b=2 c=3'
assert multi_group(1, b=2, c=3) == 'a=1 b=2 c=3'
assert multi_group(1, c=3, b=2) == 'a=1 b=2 c=3'


# === Call-site *args unpacking ===
def collect_all(*values):
    return values


source_tuple = (1, 2, 3)
assert collect_all(*source_tuple) == (1, 2, 3)

source_list = [4, 5]
assert collect_all(0, *source_list) == (0, 4, 5)
