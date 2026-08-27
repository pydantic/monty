# `partial` holds its callable, positionals and keyword values directly, so a
# missing arm in `py_dec_ref_ids` / `for_each_child_id` only shows up under
# `ref-count-return` / `memory-model-checks`.
#
# `obj` ends at 3: its binding, `bound.args`, and the tuple `called` holds.
# `target` ends at 5: its binding plus the four surviving partials that wrap it.
import functools


def target(a, b=1):
    return (a, b)


obj = [1, 2]
bound = functools.partial(target, obj)
called = bound()

# A keyword value is owned the same way as a positional one.
kw_value = {'k': 1}
keyworded = functools.partial(target, 1, b=kw_value)

# The flattened partial owns clones of the inner one's arguments, and the inner
# partial itself is released during construction.
inner_arg = [3]
flattened = functools.partial(functools.partial(target, inner_arg), 2)

# The cycle collector must reach a partial's arguments: this list holds the only
# reference to a partial that in turn holds the list.
cyclic = []
cyclic.append(functools.partial(target, cyclic))

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# `args` leaves the list alive with no referrer.
dropped = functools.partial(target, [4, 5])
dropped = None

len(called)
# ref-counts={'functools': 1, 'obj': 3, 'bound': 1, 'called': 1, 'kw_value': 2, 'keyworded': 1, 'inner_arg': 2, 'flattened': 1, 'cyclic': 2, 'target': 5}
