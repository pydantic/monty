# Test that deeply nested lists don't crash during equality comparison
# Monty raises RecursionError at depth limit, CPython handles in C code
a = []
b = []
for _ in range(30):  # Use lower depth that works within unified recursion limit
    a = [a]
    b = [b]

# Should not crash
result = a == b
assert isinstance(result, bool), 'comparison should return a bool'
assert result == True, 'structurally equal nested lists should be equal'

# Test non-equal nested lists
c = []
for _ in range(30):
    c = [c]
c = [1]  # Make the innermost different
for _ in range(29):
    c = [c]

result2 = a == c
assert result2 == False, 'structurally different nested lists should not be equal'
