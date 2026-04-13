# call-external
# === Basic external function tests ===

# Simple calls
a = add_ints(10, 20)
assert a == 30, 'add_ints basic'

b = add_ints(-5, 15)
assert b == 10, 'add_ints with negative'

s = concat_strings('hello', ' world')
assert s == 'hello world', 'concat_strings basic'

x = return_value(42)
assert x == 42, 'return_value with int'

y = return_value('test')
assert y == 'test', 'return_value with str'

# === Assignment with external calls ===
result = add_ints(100, 200)
assert result == 300, 'assignment from add_ints'

name = concat_strings('foo', 'bar')
assert name == 'foobar', 'assignment from concat_strings'

# === Nested calls ===
nested = add_ints(1, add_ints(2, 3))
assert nested == 6, 'nested add_ints right'

nested2 = add_ints(add_ints(1, 2), 3)
assert nested2 == 6, 'nested add_ints left'

nested3 = add_ints(add_ints(1, 2), add_ints(3, 4))
assert nested3 == 10, 'nested add_ints both'

deep = add_ints(add_ints(add_ints(1, 2), 3), 4)
assert deep == 10, 'deeply nested add_ints'

# === Chained operations ===
chained = add_ints(1, 2) + add_ints(3, 4)
assert chained == 10, 'chained add_ints with +'

chained2 = add_ints(10, 20) - add_ints(5, 10)
assert chained2 == 15, 'chained add_ints with -'

chained3 = add_ints(2, 3) * add_ints(4, 5)
assert chained3 == 45, 'chained add_ints with *'

str_chain = concat_strings('a', 'b') + concat_strings('c', 'd')
assert str_chain == 'abcd', 'chained concat_strings'

# === External calls in assert statements ===
assert add_ints(5, 5) == 10, 'ext call in assert condition'
assert return_value(True), 'ext call returning truthy in assert'
assert concat_strings('x', 'y') == 'xy', 'concat in assert'
assert add_ints(1, add_ints(2, 3)) == 6, 'nested ext call in assert'

# === Mixed with builtins ===
length = len(concat_strings('hello', 'world'))
assert length == 10, 'len of concat result'

items = [add_ints(1, 2), add_ints(3, 4)]
assert items[0] == 3, 'ext call in list literal first'
assert items[1] == 7, 'ext call in list literal second'

# === Multiple external calls in single expression ===

# Two ext calls added together
sum_result = add_ints(1, 2) + add_ints(3, 4)
assert sum_result == 10, 'two ext calls in addition'

# Three ext calls in one expression
triple = add_ints(1, 1) + add_ints(2, 2) + add_ints(3, 3)
assert triple == 12, 'three ext calls in expression'

# Ext calls in multiplication
mul_result = add_ints(2, 3) * add_ints(1, 1)
assert mul_result == 10, 'ext calls in multiplication'

# Ext calls in subtraction
sub_result = add_ints(10, 5) - add_ints(3, 2)
assert sub_result == 10, 'ext calls in subtraction'

# Complex expression with multiple ext calls
complex_expr = (add_ints(1, 2) + add_ints(3, 4)) * add_ints(0, 2)
assert complex_expr == 20, 'complex expr with ext calls'

# String concatenation with multiple ext calls
str_result = concat_strings(return_value('a'), return_value('b')) + concat_strings('c', 'd')
assert str_result == 'abcd', 'multiple ext calls for string concat'

# Comparison with multiple ext calls
cmp_result = add_ints(5, 5) == add_ints(3, 7)
assert cmp_result == True, 'comparison of two ext call results'

# Nested ext calls in expression
nested_expr = add_ints(add_ints(1, 2), add_ints(3, 4))
assert nested_expr == 10, 'nested ext calls in expression'
