# This test case is also used in the benchmark (benches/main.rs)
# List operations
my_list = []
my_list.append(1)
my_list.append(2)
my_list.insert(0, 0)
list_len = len(my_list)
list_item = my_list[1]

# Dict operations
my_dict = {}
my_dict['a'] = 10
my_dict['b'] = 20
dict_val = my_dict['a']
popped = my_dict.pop('b')
dict_len = len(my_dict)

# Tuple operations
my_tuple = (1, 2, 3)
tuple_item = my_tuple[0]
tuple_len = len(my_tuple)

# String operations
s = 'hello'
s += ' world'
str_len = len(s)


# Function definition and call
def add(x, y):
    return x + y


func_result = add(3, 4)

# For loop with if/elif/else
total = 0
for i in range(10):
    if i < 3:
        total += 1
    elif i < 6:
        total += 2
    else:
        total += 3

# Boolean operators and comparisons
flag = True and not False
check = 1 < 2 and 3 > 2
identity = None is None
not_identity = 1 is not None
compare = 5 >= 5 and 5 <= 5 and 4 != 5

# Assert with message
assert total > 0, 'total should be positive'

# List comprehension
squares = [x * x for x in range(10)]
comp_sum = sum(squares)

# Dict comprehension
square_dict = {x: x * x for x in range(5)}
dict_comp_sum = sum(square_dict.values())

# Final result
result = list_len + list_item + dict_val + dict_len + tuple_item + tuple_len
result += str_len + func_result + total + comp_sum + dict_comp_sum
result
# Return=373
