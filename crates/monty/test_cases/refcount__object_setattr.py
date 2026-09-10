class Point:
    pass


p = Point()
kept = [1, 2]
replaced = [3, 4]

# The written value gains a reference from the instance `__dict__`.
object.__setattr__(p, 'items', kept)

# Overwriting releases the old value and retains the new one, so `replaced`
# ends held only by its own binding.
object.__setattr__(p, 'items', replaced)
object.__setattr__(p, 'items', kept)
# ref-counts={'Point': 2, 'p': 1, 'kept': 2, 'replaced': 1}
