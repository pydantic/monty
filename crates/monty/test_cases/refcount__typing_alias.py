# A `Literal[...]` / `Annotated[...]` alias owns its argument tuple, so a
# missing arm in `py_dec_ref_ids` / `for_each_child_id` only shows up under
# `ref-count-return` / `memory-model-checks`.
#
# `choices` ends at 1: `Literal` copies the deduplicated choices into its own
# tuple rather than keeping the key, and `got` is that tuple, ending at 2.
# `meta` ends at 3: its binding, the alias's argument tuple, and the tuple
# `__metadata__` built from it.
from typing import Annotated, Literal

choices = (1, 'a')
lit = Literal[choices]
got = lit.__args__

meta = [1]
ann = Annotated[int, meta]
metadata = ann.__metadata__

# Nothing but the alias refers to this list, so the walk reaches it through
# `for_each_child_id` or not at all.
anon = Literal[[6, 7]]

# The cycle collector must reach an alias's arguments: this list holds the only
# reference to an alias that in turn holds the list.
cyclic = []
cyclic.append(Annotated[int, cyclic])
cyclic = None

# The freeing path: once the only binding goes, a `py_dec_ref_ids` that skips
# `args` leaves the list alive with no referrer.
dropped = Literal[[4, 5]]
dropped = None

len(got)
# ref-counts={'choices': 1, 'lit': 1, 'got': 2, 'meta': 3, 'ann': 1, 'metadata': 1, 'anon': 1}
