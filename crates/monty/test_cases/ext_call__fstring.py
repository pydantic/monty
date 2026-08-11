# call-external
# External calls in f-strings

s = f'result is {add_ints(10, 20)}'
assert s == 'result is 30'

s = f'a={add_ints(1, 2)}, b={add_ints(3, 4)}'
assert s == 'a=3, b=7'

# Nested external call in f-string
s = f'nested={add_ints(add_ints(1, 2), 3)}'
assert s == 'nested=6'
