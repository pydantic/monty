# Calling an unknown method still owns the arguments the call evaluated, so the
# AttributeError path has to release them. Regression test: sandboxed code could
# otherwise leak a reference per call in a loop.
from datetime import time

lst = [1, 2, 3]

for _ in range(3):
    try:
        time(1).bogus(lst)
        assert False, 'expected AttributeError'
    except AttributeError:
        pass
# ref-counts={'lst': 1}
