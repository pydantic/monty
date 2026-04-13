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

# Exercise negative count path (early return)
result = p.sub('repl', 'hello', -1)
assert result == 'hello', 'negative count returns input unchanged'

# repl_list: 1 (variable)
# input_list: 1 (variable)
# p: 1 (variable)
# re: 1 (module)
# result: 2 (variable + final expression)
result
# ref-counts={'repl_list': 1, 'input_list': 1, 'p': 1, 're': 1, 'result': 2}
