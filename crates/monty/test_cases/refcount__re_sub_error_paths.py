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

# Exercise negative count path (early return, no regex compilation)
result = re.sub('pattern', 'repl', 'hello', -1)
assert result == 'hello', 'negative count returns input unchanged'

# All lists should still be alive and reachable
# repl_list: 1 (variable)
# input_list: 1 (variable)
# re: 1 (module)
# result: 2 (variable + final expression)
result
# ref-counts={'repl_list': 1, 'input_list': 1, 're': 1, 'result': 2}
