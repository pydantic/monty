# A Python callback on every iteration exercises the frozen reduce loop.
import functools


def add(a, b):
    return a + b


assert functools.reduce(add, range(10_000)) == 49_995_000
