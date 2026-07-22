# `in` clones the needle to hand it to `__contains__`, which owns its argument,
# and drops whatever the method returns. Neither may leak, on either path: when
# the class defines `__contains__`, and when the clone has to be released again
# because it does not and `in` falls back to iteration.
class Always:
    def __contains__(self, item):
        return item


class IterOnly:
    def __iter__(self):
        return iter([1])


needle = [1, 2]
a = Always()
i = IterOnly()

for _ in range(3):
    assert needle in a
    assert needle not in i

needle
# `needle` is 2 — its binding plus the trailing expression's returned value; a
# leaked clone would push it to 5, one per loop iteration. The classes are 2:
# the global name plus their instance's class slot.
# ref-counts={'needle': 2, 'a': 1, 'i': 1, 'Always': 2, 'IterOnly': 2}
