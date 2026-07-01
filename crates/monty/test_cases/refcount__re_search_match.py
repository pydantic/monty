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
# so strict matching passes (all heap objects must be reachable).
# Use multi-char tokens so each result is heap-allocated; single-ASCII results
# would be interned (see allocate_string), and interned values don't appear in
# the ref-counts map.
results = p.findall('aa bb cc')
assert results == ['aa', 'bb', 'cc'], 'findall returns list of matches'
r0 = results[0]
r1 = results[1]
r2 = results[2]

# === Module-level error paths ===
# Once the subject has been pulled out of the positional-arg iterator its
# guard no longer covers it, so each error path below must drop it manually.
# (Concatenation defeats literal interning, so subject is a real heap string.)
subject = 'hello' + ' world'

# Non-string pattern: arity is validated first, then pattern conversion fails
try:
    re.search(123, subject)
except TypeError:
    pass

# Bad flags type: extract_flags fails after the subject was extracted
try:
    re.search('h', subject, 'bad')
except TypeError:
    pass

# Too many positional args: the extra value and the subject are both dropped
try:
    re.search('h', subject, 0, subject)
except TypeError:
    pass

# p: 1, m: 1, group_str: 1, m2: 1, full_str: 1
# results: 1, r0: 2 (var + list), r1: 2 (var + list), r2: 2 (var + list + final expr)
# subject: 1 (all error paths dropped their borrowed copies)
# re: 1
r2
# ref-counts={'p': 1, 'm': 1, 'group_str': 1, 'm2': 1, 'full_str': 1, 'results': 1, 'r0': 2, 'r1': 2, 'r2': 3, 'subject': 1, 're': 1}
