# A `types.GenericAlias` owns its `__args__` tuple, so a missing arm in
# `py_dec_ref_ids` / `for_each_child_id` only shows up under
# `ref-count-return` / `memory-model-checks`.
#
# `args` and `got` are the same tuple, ending at 3: the binding of each and
# the alias holding it as `__args__`.
# `item` ends at 2: its binding and the one-item tuple `single` wraps it in.
args = (int, str)
pair = dict[args]
got = pair.__args__

item = [1]
single = list[item]

# Nothing but the alias refers to this tuple's items, so the walk reaches them
# through `for_each_child_id` or not at all.
anon = list[[6, 7]]

# The cycle collector must reach an alias's arguments: this list holds the only
# reference to an alias that in turn holds the list.
cyclic = []
cyclic.append(list[cyclic])
cyclic = None

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# `args` leaves the list alive with no referrer.
dropped = list[[4, 5]]
dropped = None

len(got)
# ref-counts={'args': 3, 'pair': 1, 'got': 3, 'item': 2, 'single': 1, 'anon': 1}
