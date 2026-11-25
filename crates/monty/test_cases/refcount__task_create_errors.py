# create_task must release its coroutine when an explicit-name __str__ or an
# invalid-argument __repr__ raises before the TaskFuture is allocated.
import asyncio


class BadName:
    def __str__(self):
        raise ValueError('bad name')


class BadRepr:
    def __repr__(self):
        raise ValueError('bad repr')


async def child():
    return 1


async def main():
    coro = child()
    try:
        asyncio.create_task(coro, name=BadName())
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'bad name'
    coro = None

    value = BadRepr()
    try:
        asyncio.create_task(value)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'bad repr'
    value = None

    # A heap-allocated name must be released when the coroutine is rejected.
    name = [1, 2]
    try:
        asyncio.create_task(1, name=name)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'a coroutine was expected, got 1'
    name = None


asyncio.run(main())
# ref-counts={'BadName': 1, 'asyncio': 1, 'BadRepr': 1}
