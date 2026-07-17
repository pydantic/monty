# Tests reference counting for re.search, re.match, and re.fullmatch.
#
# Verifies that Match objects, Pattern objects, and intermediate strings
# are correctly reference-counted through normal usage paths.
# All heap objects must be directly referenced by variables for strict matching.

import re

# Compile a pattern and run search — both pattern and match stay alive
p = re.compile(r'(\w+)')
m = p.search('hello world')
assert m is not None
group_str = m.group(0)
assert group_str == 'hello'

# Run fullmatch — exercises the compiled_fullmatch regex path
m2 = p.fullmatch('hello')
assert m2 is not None
full_str = m2.group(0)
assert full_str == 'hello'

# findall returns a list — keep individual elements in variables
# so strict matching passes (all heap objects must be reachable).
# Use multi-char tokens so each result is heap-allocated; single-ASCII results
# would be interned (see allocate_string), and interned values don't appear in
# the ref-counts map.
results = p.findall('aa bb cc')
assert results == ['aa', 'bb', 'cc']
r0 = results[0]
r1 = results[1]
r2 = results[2]

# === Module-level error paths ===
# The FromArgs slots guard drops extracted values when binding fails, and the
# body coercions (resolve_pattern / extract_count / subject_str) must drop
# their peers on each failure path.
# (Concatenation defeats literal interning, so subject is a real heap string.)
subject = 'hello' + ' world'

# Non-string pattern: binding succeeds, pattern coercion fails in the body —
# the subject is dropped by its defer_drop guard
try:
    re.search(123, subject)
    assert False, 'expected re.search with int pattern to raise TypeError'
except TypeError:
    pass

# Bad flags type: pattern already coerced, flags coercion fails
try:
    re.search('h', subject, 'bad')
    assert False, 'expected re.search with str flags to raise TypeError'
except TypeError:
    pass

# Too many positional args: binding fails, the slots guard drops the already-
# bound subject and the overflow value
try:
    re.search('h', subject, 0, subject)
    assert False, 'expected re.search with 4 positional args to raise TypeError'
except TypeError:
    pass

# Compiled pattern with flags: the ValueError path must drop the still-live
# compiled-pattern reference taken at extraction
try:
    re.search(p, subject, 1)
    assert False, 'expected re.search with compiled pattern and flags to raise ValueError'
except ValueError:
    pass

# Compiled pattern used by a module function: p's refcount returns to 1
m3 = re.search(p, subject)
assert m3 is not None

# Negative count returns the subject: subject gains a reference from the result
sub_result = re.sub('h', 'X', subject, -1)
assert sub_result == 'hello world'

# p: 1, m: 1, group_str: 1, m2: 1, full_str: 1
# results: 1, r0: 2 (var + list), r1: 2 (var + list), r2: 2 (var + list + final expr)
# subject: 3 (variable + sub_result aliasing it + m3's retained .string reference)
# sub_result: 3 (same object as subject)
# m3: 1 (its shared reference to subject is counted under subject above)
# re: 1
r2
# ref-counts={'p': 1, 'm': 1, 'group_str': 1, 'm2': 1, 'full_str': 1, 'results': 1, 'r0': 2, 'r1': 2, 'r2': 3, 'subject': 3, 'sub_result': 3, 'm3': 1, 're': 1}
