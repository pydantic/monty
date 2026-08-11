# call-external
# === Basic external function tests ===

# Simple calls
a = add_ints(10, 20)
assert a == 30

b = add_ints(-5, 15)
assert b == 10

s = concat_strings('hello', ' world')
assert s == 'hello world'

x = return_value(42)
assert x == 42

y = return_value('test')
assert y == 'test'

# === Assignment with external calls ===
result = add_ints(100, 200)
assert result == 300

name = concat_strings('foo', 'bar')
assert name == 'foobar'

# === Nested calls ===
nested = add_ints(1, add_ints(2, 3))
assert nested == 6

nested2 = add_ints(add_ints(1, 2), 3)
assert nested2 == 6

nested3 = add_ints(add_ints(1, 2), add_ints(3, 4))
assert nested3 == 10

deep = add_ints(add_ints(add_ints(1, 2), 3), 4)
assert deep == 10

# === Chained operations ===
chained = add_ints(1, 2) + add_ints(3, 4)
assert chained == 10

chained2 = add_ints(10, 20) - add_ints(5, 10)
assert chained2 == 15

chained3 = add_ints(2, 3) * add_ints(4, 5)
assert chained3 == 45

str_chain = concat_strings('a', 'b') + concat_strings('c', 'd')
assert str_chain == 'abcd'

# === External calls in assert statements ===
assert add_ints(5, 5) == 10
assert return_value(True)
assert concat_strings('x', 'y') == 'xy'
assert add_ints(1, add_ints(2, 3)) == 6

# === Mixed with builtins ===
length = len(concat_strings('hello', 'world'))
assert length == 10

items = [add_ints(1, 2), add_ints(3, 4)]
assert items[0] == 3
assert items[1] == 7

# === Multiple external calls in single expression ===

# Two ext calls added together
sum_result = add_ints(1, 2) + add_ints(3, 4)
assert sum_result == 10

# Three ext calls in one expression
triple = add_ints(1, 1) + add_ints(2, 2) + add_ints(3, 3)
assert triple == 12

# Ext calls in multiplication
mul_result = add_ints(2, 3) * add_ints(1, 1)
assert mul_result == 10

# Ext calls in subtraction
sub_result = add_ints(10, 5) - add_ints(3, 2)
assert sub_result == 10

# Complex expression with multiple ext calls
complex_expr = (add_ints(1, 2) + add_ints(3, 4)) * add_ints(0, 2)
assert complex_expr == 20

# String concatenation with multiple ext calls
str_result = concat_strings(return_value('a'), return_value('b')) + concat_strings('c', 'd')
assert str_result == 'abcd'

# Comparison with multiple ext calls
cmp_result = add_ints(5, 5) == add_ints(3, 7)
assert cmp_result == True

# Nested ext calls in expression
nested_expr = add_ints(add_ints(1, 2), add_ints(3, 4))
assert nested_expr == 10
