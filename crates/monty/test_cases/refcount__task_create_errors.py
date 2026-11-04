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


asyncio.run(main())
# ref-counts={'BadName': 1, 'asyncio': 1, 'BadRepr': 1}
