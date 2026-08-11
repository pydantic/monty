# Tests reference counting on re.sub error paths.
#
# The positional arg iterator and extra args must be properly dropped even
# when re.sub raises due to too many args or a bad count type.
# These paths previously leaked because pos.next().is_some() consumed a
# Value without dropping it, and the pos iterator itself was unguarded.

import re

# Use lists as heap-allocated values that we can track through error paths.
# String literals may be interned and won't show up in heap ref counts.
repl_list = ['replacement']
input_list = ['the input']

# Exercise error path: bad count type with heap-allocated args in scope
try:
    re.sub('pattern', 'repl', 'input', 'bad')
except TypeError:
    pass

# Exercise error path: pattern compilation fails while count/maxsplit hold
# heap values. resolve_pattern errors before count/maxsplit are consumed, so
# they must be guarded rather than left owned across the `?`.
count_list = [1]
try:
    re.sub('(', 'repl', 'input', count_list)
except re.PatternError:
    pass
maxsplit_list = [1]
try:
    re.split('(', 'input', maxsplit_list)
except re.PatternError:
    pass

# Same early-error shape via flags: a compiled pattern with truthy flags
# raises ValueError inside resolve_pattern, again before count is consumed.
compiled = re.compile('pattern')
try:
    re.sub(compiled, 'repl', 'input', count_list, 4)
except ValueError:
    pass

# Negative count path with an INTERNED input: the negative-count short-circuit
# returns the value untouched (refcount-bumped), so an interned input stays
# interned — no heap allocation, no entry in the refcount map.
interned_result = re.sub('pattern', 'repl', 'hello', -1)
assert interned_result == 'hello'

# Negative count path with a HEAP-allocated input: the short-circuit shares the
# same heap object back to the caller, so input_str and result alias each other.
# (Concatenation at runtime defeats compile-time literal interning.)
input_str = 'hel' + 'lo'
result = re.sub('pattern', 'repl', input_str, -1)
assert result == 'hello'

# All lists should still be alive and reachable.
# repl_list: 1 (variable)
# input_list: 1 (variable)
# count_list / maxsplit_list: 1 each (leak regression: 2+ if an error path drops them late)
# compiled: 1 (variable)
# re: 1 (module)
# interned_result: not heap-allocated, absent from the map
# input_str and result reference the same heap string: 2 vars + final expr = 3
result
# ref-counts={'repl_list': 1, 'input_list': 1, 'count_list': 1, 'maxsplit_list': 1, 'compiled': 1, 're': 1, 'input_str': 3, 'result': 3}
