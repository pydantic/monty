# Reference counts stay balanced through the `with` machinery's pushed
# frames: `BeforeWith` pushes the ctx and the `__enter__` frame binds self;
# `WithExit`/`WithExceptStart` bind self plus the exception triple. The
# suppressed exception survives through the attribute written by `__exit__`;
# we bind it to `survived` at the end so the strict ref-count check (which
# requires every live heap object to be reachable from a named variable)
# accounts for it rather than seeing an unreferenced third heap object.
class CM:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        self.last_exc = val
        return True


cm = CM()
with cm as bound:
    pass

with cm:
    raise ValueError('kept alive via cm.last_exc')

# The suppressed exception is held by `cm.last_exc` (refcount 1) plus this
# binding (refcount 2); `cm` and `bound` alias the one instance (refcount 2);
# `CM` is held by the global plus the instance's class slot (refcount 2).
survived = cm.last_exc
# The class's synthesized `__annotations__` dict (empty here) is a heap object
# owned by the class namespace, so bind it too — see `refcount__class.py`.
ann = CM.__annotations__
# ref-counts={'CM': 2, 'cm': 2, 'bound': 2, 'survived': 2, 'ann': 2}
