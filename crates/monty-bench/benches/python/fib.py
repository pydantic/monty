# Recursive function calls: `fib(25)`.

def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)


assert fib(25) == 75_025
