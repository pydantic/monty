# Reference counts stay balanced through the `with` machinery's pushed
# frames: `BeforeWith` pushes the ctx and the `__enter__` frame binds self;
# `WithExit`/`WithExceptStart` bind self plus the exception triple. The
# suppressed exception below survives only through the attribute written
# by `__exit__`.
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
# ref-counts={'CM': 2, 'cm': 2, 'bound': 2}
