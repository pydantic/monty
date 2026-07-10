# call-external
# run-async
# An __init__ that suspends while the module is driven in async mode: the
# external call inside __init__ goes through the pending-futures path and
# resumes mid-construction (the async-mode counterpart of
# class__init_external.py).


class Accumulator:
    def __init__(self, base: int) -> None:
        # `add_ints` is an external function resolved by the host.
        self.total = add_ints(base, 100)
        self.base = base


start = await async_call(7)  # pyright: ignore
acc = Accumulator(start)
assert acc.base == 7
assert acc.total == 107
assert type(acc) is Accumulator

# === awaiting a non-awaitable instance names the class ===
try:
    await acc  # pyright: ignore
    assert False, 'expected await to fail'
except TypeError as exc:
    assert str(exc) == "'Accumulator' object can't be awaited"
