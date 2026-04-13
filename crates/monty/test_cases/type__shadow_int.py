# Builtin type name 'int' can be shadowed by assignment
int = 42
assert int == 42, 'int shadowed by assignment'

# for loop variable shadows builtin
result = []
for int in range(3):
    result.append(int)
assert result == [0, 1, 2], 'int works as for loop variable'
