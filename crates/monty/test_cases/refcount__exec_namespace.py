# An explicit namespace dict is held by every function defined under it (a
# plain def, one with defaults, and a method) and released with them; a value
# the snippet binds is held by the dict entry.
ns = {}
exec('def f():\n    return 1\ndef g(x=1):\n    return x\nclass C:\n    def m(self):\n        return 2', ns)
obj = [1]
exec('v = obj', {'obj': obj}, ns)

# The functions in a namespace dict hold the dict: a cycle the collector
# must traverse to free everything once the last outside reference goes.
import gc

cyc = {}
exec('def f():\n    return 1', cyc)
cyc = None
gc.collect()
# ref-counts={'ns': 4, 'obj': 2, 'gc': 1}
