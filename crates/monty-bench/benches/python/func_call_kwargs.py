# A single call binding a keyword argument and a default.

def add(a, b=2):
    return a + b


assert add(a=1) == 3
