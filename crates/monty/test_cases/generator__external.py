# /// monty
# call-external = true
# run-async = true
# ///
# Host calls inside running generators: each suspends the VM with the generator's
# frame on the stack, and the harness dumps and reloads the session at every call.

# === A host call in the generator body ===


def doubled(values):
    for value in values:
        yield add_ints(value, value)


assert next(doubled([1])) == 2
assert [v for v in doubled([1, 2, 3])] == [2, 4, 6]
total = 0
for v in doubled([5, 6]):
    total = add_ints(total, v)
assert total == 22
g = doubled([10, 20])
assert next(g) == 20
assert next(g, None) == 40
assert next(g, 'done') == 'done'

# === Host calls before the first yield and after the last ===


def bracketed():
    start = return_value('start')
    yield start
    yield return_value('middle')
    return return_value('end')


g = bracketed()
assert next(g) == 'start'
assert next(g) == 'middle'
try:
    next(g)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value == 'end'

# === A host call in a yield from delegate ===
shared = []


def inner():
    yield add_ints(40, 2)
    return shared


def middle():
    return (yield from inner())


def outer():
    result = yield from middle()
    assert result is shared
    yield 'after'


assert list(o for o in outer()) == [42, 'after']


class HostIterator:
    def __init__(self):
        self.done = False

    def __iter__(self):
        return self

    def __next__(self):
        if self.done:
            raise StopIteration(shared)
        self.done = True
        return add_ints(1, 2)


def from_host_iterator():
    result = yield from HostIterator()
    assert result is shared
    yield 'after'


g = from_host_iterator()
assert next(g) == 3
assert next(g) == 'after'
assert next(g, None) is None


def from_map():
    yield from map(lambda x: add_ints(x, 1), [1, 2])


g = from_map()
assert next(g) == 2
assert next(g) == 3
assert next(g, None) is None

# === A host error raised inside a generator ===


def failing():
    try:
        raise_error('ValueError', 'host failed')
    except ValueError as exc:
        yield str(exc)


assert next(failing()) == 'host failed'

# === Generators used from an async function ===


async def consume():
    results = []
    for value in doubled([7]):
        results.append(value)
    results.append(await async_call(next(doubled([8]))))
    return results


assert await consume() == [14, 16]  # pyright: ignore

# === StopIteration escaping a coroutine becomes RuntimeError (PEP 479) ===


def exhausted():
    return
    yield


async def advances(it):
    await async_call(None)
    next(it)


async def raises_stop():
    raise StopIteration(3)


async def catches_stop():
    try:
        raise StopIteration('inner')
    except StopIteration as e:
        return e.value


async def coroutine_stop_iteration():
    results = []
    for coro in [advances(exhausted()), raises_stop()]:
        try:
            await coro
            assert False, 'expected RuntimeError'
        except RuntimeError as e:
            results.append((str(e), type(e.__cause__).__name__))
    results.append(await catches_stop())
    return results


assert await coroutine_stop_iteration() == [  # pyright: ignore
    ('coroutine raised StopIteration', 'StopIteration'),
    ('coroutine raised StopIteration', 'StopIteration'),
    'inner',
]
