# /// monty
# call-external = true
# run-async = true
# ///
# Native continuations inside concurrent tasks: each task suspends on host calls inside `__next__`.
import asyncio


class Counter:
    def __init__(self, stop, offset):
        self.n = 0
        self.stop = stop
        self.offset = offset

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > self.stop:
            raise StopIteration
        return add_ints(self.n, self.offset)


async def worker(stop, offset):
    first = list(Counter(stop, offset))
    await async_call(None)
    total = 0
    for x in Counter(stop, offset):
        total += x
    await async_call(None)
    return first, total, sum(Counter(stop, offset)), tuple(Counter(stop, offset))


results = await asyncio.gather(worker(2, 10), worker(3, 100), worker(0, 5))  # pyright: ignore
assert results == [
    ([11, 12], 23, 23, (11, 12)),
    ([101, 102, 103], 306, 306, (101, 102, 103)),
    ([], 0, 0, ()),
]
