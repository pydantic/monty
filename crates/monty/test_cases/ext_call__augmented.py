# call-external
# External calls in augmented assignment expressions

# += with external call
x = 10
x += add_ints(5, 5)
assert x == 20, 'ext call in augmented add'

# -= with external call
x = 100
x -= add_ints(20, 30)
assert x == 50, 'ext call in augmented sub'

# *= with external call
x = 5
x *= add_ints(2, 1)
assert x == 15, 'ext call in augmented mul'
