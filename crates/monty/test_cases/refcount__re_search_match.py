# Tests reference counting for re.search, re.match, and re.fullmatch.
#
# Verifies that Match objects, Pattern objects, and intermediate strings
# are correctly reference-counted through normal usage paths.
# All heap objects must be directly referenced by variables for strict matching.

import re

# Compile a pattern and run search — both pattern and match stay alive
p = re.compile(r'(\w+)')
m = p.search('hello world')
assert m is not None, 'search finds match'
group_str = m.group(0)
assert group_str == 'hello', 'group(0) returns matched text'

# Run fullmatch — exercises the compiled_fullmatch regex path
m2 = p.fullmatch('hello')
assert m2 is not None, 'fullmatch finds match'
full_str = m2.group(0)
assert full_str == 'hello', 'fullmatch group(0) returns matched text'

# findall returns a list — keep individual elements in variables
# so strict matching passes (all heap objects must be reachable)
results = p.findall('a b c')
assert results == ['a', 'b', 'c'], 'findall returns list of matches'
r0 = results[0]
r1 = results[1]
r2 = results[2]

# p: 1, m: 1, group_str: 1, m2: 1, full_str: 1
# results: 1, r0: 2 (var + list), r1: 2 (var + list), r2: 2 (var + list + final expr)
# re: 1
r2
# ref-counts={'p': 1, 'm': 1, 'group_str': 1, 'm2': 1, 'full_str': 1, 'results': 1, 'r0': 2, 'r1': 2, 'r2': 3, 're': 1}
