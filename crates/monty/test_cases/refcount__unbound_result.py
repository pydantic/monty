# The module's trailing expression is an unbound heap value. It is still owned by
# the pending frame exit when ref-counts are read, so the strict check must treat
# it as accounted for — otherwise it is falsely reported as an unreferenced heap
# object. `a` is 2 here: its own binding plus the returned list's reference to it.
a = [1]
[a, 2]
# ref-counts={'a': 2}
