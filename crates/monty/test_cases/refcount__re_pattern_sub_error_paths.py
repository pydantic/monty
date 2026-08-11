# Tests reference counting on Pattern.sub error paths.
#
# The positional arg iterator and extra args must be properly dropped even
# when Pattern.sub raises due to too many args or a bad count type.
# These paths previously leaked because pos.next().is_some() consumed a
# Value without dropping it.

import re

# Use lists as heap-allocated values we can track
repl_list = ['replacement']
input_list = ['the input']
p = re.compile('hello')

# Exercise error path: too many positional arguments
try:
    p.sub('repl', 'string', 0, 'extra')
except TypeError:
    pass

# Exercise error path: bad count type
try:
    p.sub('repl', 'string', 'bad')
except TypeError:
    pass

# Negative count path with an INTERNED input: the negative-count short-circuit
# returns the value untouched (refcount-bumped), so an interned input stays
# interned — no heap allocation, no entry in the refcount map.
interned_result = p.sub('repl', 'hello', -1)
assert interned_result == 'hello'

# Negative count path with a HEAP-allocated input: the short-circuit shares the
# same heap object back to the caller, so input_str and result alias each other.
# (Concatenation at runtime defeats compile-time literal interning.)
input_str = 'hel' + 'lo'
result = p.sub('repl', input_str, -1)
assert result == 'hello'

# repl_list: 1 (variable)
# input_list: 1 (variable)
# p: 1 (variable)
# re: 1 (module)
# interned_result: not heap-allocated, absent from the map
# input_str and result reference the same heap string: 2 vars + final expr = 3
result
# ref-counts={'repl_list': 1, 'input_list': 1, 'p': 1, 're': 1, 'input_str': 3, 'result': 3}
