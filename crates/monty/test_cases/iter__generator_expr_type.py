# xfail=cpython
# TODO: When proper generators are implemented, this test should be removed.
# Currently generator expressions return lists in Monty, not generator objects.
# This test verifies the temporary behavior until generators are properly implemented.
gen_result = (x * 2 for x in range(5))
assert type(gen_result) == list, 'generator expr currently returns list'
assert gen_result == [0, 2, 4, 6, 8], 'generator expr value'
