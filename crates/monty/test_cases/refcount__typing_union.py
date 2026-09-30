# A `typing.Union` owns its `__args__` tuple, so a missing arm in
# `py_dec_ref_ids` / `for_each_child_id` only shows up under
# `ref-count-return` / `memory-model-checks`.
#
# `alias` ends at 2: its binding and the `__args__` tuple of `maybe`.
# `args` and `got` are the same tuple, ending at 3: the binding of each and
# the union holding it as `__args__`.
alias = list[int]
maybe = alias | None
args = maybe.__args__
got = maybe.__args__

# Flattening clones the inner union's members and releases the inner union
# itself, which nothing else refers to.
wide = (int | str) | bytes

# Nothing but the union refers to this alias, so the walk reaches it through
# `for_each_child_id` or not at all.
anon = list[[1, 2]] | None

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# `args` leaves the alias alive with no referrer.
dropped = list[[3]] | None
dropped = None

len(got)
# ref-counts={'alias': 2, 'maybe': 1, 'args': 3, 'got': 3, 'wide': 1, 'anon': 1}
