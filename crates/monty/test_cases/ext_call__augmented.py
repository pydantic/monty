# call-external
# External calls in augmented assignment expressions

# += with external call
x = 10
x += add_ints(5, 5)
assert x == 20

# -= with external call
x = 100
x -= add_ints(20, 30)
assert x == 50

# *= with external call
x = 5
x *= add_ints(2, 1)
assert x == 15
