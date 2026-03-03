# call-external
# Tests for NameLookup resolution with various value types.
# Verifies that the host can inject non-function values (constants)
# into the sandbox namespace via the NameLookup mechanism.

# === Integer constant ===
assert CONST_INT == 42, f'CONST_INT should be 42, got {CONST_INT}'
assert CONST_INT + 8 == 50, 'CONST_INT arithmetic'
assert type(CONST_INT) == int, f'CONST_INT type should be int, got {type(CONST_INT)}'

# === String constant ===
assert CONST_STR == 'hello', f'CONST_STR should be hello, got {CONST_STR}'
assert CONST_STR + ' world' == 'hello world', 'CONST_STR concatenation'
assert len(CONST_STR) == 5, f'CONST_STR length should be 5, got {len(CONST_STR)}'
assert type(CONST_STR) == str, f'CONST_STR type should be str, got {type(CONST_STR)}'

# === Float constant ===
assert CONST_FLOAT == 3.14, f'CONST_FLOAT should be 3.14, got {CONST_FLOAT}'
assert CONST_FLOAT + 0.86 == 4.0, 'CONST_FLOAT arithmetic'
assert type(CONST_FLOAT) == float, f'CONST_FLOAT type should be float, got {type(CONST_FLOAT)}'

# === Boolean constant ===
assert CONST_BOOL == True, f'CONST_BOOL should be True, got {CONST_BOOL}'
assert CONST_BOOL and True, 'CONST_BOOL in boolean expression'
assert type(CONST_BOOL) == bool, f'CONST_BOOL type should be bool, got {type(CONST_BOOL)}'

# === List constant ===
assert CONST_LIST == [1, 2, 3], f'CONST_LIST should be [1, 2, 3], got {CONST_LIST}'
assert len(CONST_LIST) == 3, f'CONST_LIST length should be 3, got {len(CONST_LIST)}'
assert CONST_LIST[0] == 1, 'CONST_LIST first element'
assert CONST_LIST[-1] == 3, 'CONST_LIST last element'
assert type(CONST_LIST) == list, f'CONST_LIST type should be list, got {type(CONST_LIST)}'

# === None constant ===
assert CONST_NONE is None, f'CONST_NONE should be None, got {CONST_NONE}'
assert type(CONST_NONE) == type(None), f'CONST_NONE type should be NoneType, got {type(CONST_NONE)}'

# === Caching: same constant used twice should work ===
x = CONST_INT
y = CONST_INT
assert x == y == 42, 'cached CONST_INT should be consistent'

# === Mixed: constants and external functions in the same code ===
result = add_ints(CONST_INT, 8)
assert result == 50, f'add_ints(CONST_INT, 8) should be 50, got {result}'

str_result = concat_strings(CONST_STR, ' world')
assert str_result == 'hello world', f'concat with CONST_STR should be hello world, got {str_result}'

# === Constants used in control flow ===
if CONST_BOOL:
    flag = 'yes'
else:
    flag = 'no'
assert flag == 'yes', f'CONST_BOOL in if should take true branch, got {flag}'

# === Constants used in loops ===
total = 0
for item in CONST_LIST:
    total = total + item
assert total == 6, f'sum of CONST_LIST should be 6, got {total}'


# === Constants in function scope ===
def use_constant():
    return CONST_INT * 2


assert use_constant() == 84, f'CONST_INT in function should be 84, got {use_constant()}'
