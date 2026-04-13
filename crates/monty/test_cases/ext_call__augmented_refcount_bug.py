# call-external
# BUG: Reference counting bug with string augmented assignment and external calls

# String += with external call causes reference counting error
s = 'hello'
s += concat_strings(' ', 'world')
assert s == 'hello world', 'ext call in augmented string concat'
