# The implicit `__class__` closure cell creates a `Class -> Closure -> Cell -> Class`
# reference cycle for any class whose methods reference `super` or `__class__`.
class LiveBase:
    def v(self):
        return 1


class LiveChild(LiveBase):
    def v(self):
        return super().v() + 1

    def cls(self):
        return __class__


def create_temporary_cycles():
    class TempBase:
        def v(self):
            return 10

    class TempChild(TempBase):
        def v(self):
            return (lambda s: super().v() + 5)(self)

        def who(self):
            return __class__

    tc = TempChild()
    assert tc.v() == 15
    assert tc.who() is TempChild
    # Also form an Instance <-> Super cycle so GC traverses `Super::py_dec_ref_ids`.
    tc.sup = super(TempChild, tc)

    class RejectingBase:
        def __init_subclass__(cls):
            raise ValueError('boom')

    try:

        class FailedChild(RejectingBase):
            def who(self):
                return __class__

    except ValueError:
        pass


create_temporary_cycles()
inst = LiveChild()
assert inst.v() == 2
assert inst.cls() is LiveChild
# `LiveBase`: global + `LiveChild.bases`.
# `LiveChild`: global + the shared `__class__` cell + `inst`'s class reference.
# ref-counts={'LiveBase': 2, 'LiveChild': 3, 'inst': 1}
