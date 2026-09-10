# call-external
# Calling a `partial` goes through the ordinary call path, so a wrapped host
# function still suspends to the host with the merged arguments — unlike
# `reduce`, whose function runs in a context that cannot suspend (see
# limitations/functools.md).
import functools

add_one = functools.partial(add_ints, 1)
assert add_one(2) == 3
assert add_one(10) == 11

# The bound argument survives repeated calls and mixes with later positionals.
assert functools.partial(add_ints, 20)(22) == 42
assert functools.partial(concat_strings, 'a')('b') == 'ab'
