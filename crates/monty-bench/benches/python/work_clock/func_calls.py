def add(a, b):
    return a + b


def fib(n):
    return n if n < 2 else add(fib(n - 1), fib(n - 2))


assert fib(22) == 17711
