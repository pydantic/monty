# Builtin exception type 'ValueError' can be shadowed by assignment
ValueError = 'not an exception'
assert ValueError == 'not an exception', 'ValueError shadowed'
