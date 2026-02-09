def recurse():
    recurse()


try:
    recurse()
except RecursionError:
    pass

print('caught recursion')
