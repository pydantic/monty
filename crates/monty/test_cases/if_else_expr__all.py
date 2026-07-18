# === Basic if/else ===
assert (1 if True else 2) == 1
assert (1 if False else 2) == 2

# === Truthy/falsy values ===
assert ('yes' if 1 else 'no') == 'yes'
assert ('yes' if 0 else 'no') == 'no'
assert ('yes' if 'a' else 'no') == 'yes'
assert ('yes' if '' else 'no') == 'no'
assert ('yes' if [1] else 'no') == 'yes'
assert ('yes' if [] else 'no') == 'no'
assert ('yes' if None else 'no') == 'no'

# === Variables and comparisons ===
x = 5
assert (x if x > 0 else -x) == 5
x = -3
assert (x if x > 0 else -x) == 3

# === Nested if/else ===
a = 1
b = 2
c = 3
assert ((a if a > b else b) if True else c) == 2
assert ((a if a > b else b) if False else c) == 3
assert (a if True else (b if True else c)) == 1

# === Complex expressions ===
assert (1 + 2 if True else 3 + 4) == 3
assert (1 + 2 if False else 3 + 4) == 7

# === With heap values (strings, lists) ===
s1 = 'hello'
s2 = 'world'
assert (s1 if True else s2) == 'hello'
assert (s1 if False else s2) == 'world'

l1 = [1, 2]
l2 = [3, 4]
result = l1 if True else l2
assert result == [1, 2]
result = l1 if False else l2
assert result == [3, 4]

# === In f-strings ===
val = 10
assert f'{val if val > 5 else 0}' == '10'
val = 3
assert f'{val if val > 5 else 0}' == '0'
assert f'value: {1 if True else 2}' == 'value: 1'
assert f'{"yes" if 1 else "no"}' == 'yes'

# === F-string with format spec ===
x = 42
assert f'{x if True else 0:05d}' == '00042'
