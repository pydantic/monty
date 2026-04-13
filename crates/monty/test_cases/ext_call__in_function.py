# call-external
# === External function calls inside user-defined functions ===

# Basic function calling external function
def add_wrapper(a, b):
    return add_ints(a, b)


result = add_wrapper(10, 20)
assert result == 30, 'basic ext call in function'


# Function with multiple external calls (sequential)
def multi_ext():
    x = add_ints(1, 2)
    y = add_ints(3, 4)
    return add_ints(x, y)


assert multi_ext() == 10, 'multiple ext calls in function'


# External call in function with local variable usage
def with_locals():
    x = 100
    y = add_ints(x, 50)
    z = y * 2
    return z


assert with_locals() == 300, 'ext call with locals'


# Function returning external call result
def get_sum(a, b, c):
    temp = add_ints(a, b)
    return add_ints(temp, c)


assert get_sum(1, 2, 3) == 6, 'chained ext calls in function'
