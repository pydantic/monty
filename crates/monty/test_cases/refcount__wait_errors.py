# Rejected wait() calls must not leak the values collected before the error.
import asyncio


async def child():
    return 1


async def main():
    task = asyncio.create_task(child())
    await task

    try:
        await asyncio.wait([task], return_when='INVALID')
        raise AssertionError('expected ValueError')
    except ValueError:
        pass

    try:
        await asyncio.wait(task)
        raise AssertionError('expected TypeError')
    except TypeError:
        pass

    try:
        await asyncio.wait(42)
        raise AssertionError('expected TypeError')
    except TypeError:
        pass

    coro = child()
    try:
        await asyncio.wait([coro])
        raise AssertionError('expected TypeError')
    except TypeError:
        pass
    assert await coro == 1

    try:
        await asyncio.wait([task, 42])
        raise AssertionError('expected AttributeError')
    except AttributeError:
        pass

    # The rejected item comes before values that must still be released.
    leftover = asyncio.create_task(child())
    try:
        await asyncio.wait([42, leftover])
        raise AssertionError('expected AttributeError')
    except AttributeError:
        pass
    assert await leftover == 1

    middle_first = asyncio.create_task(child())
    middle_last = asyncio.create_task(child())
    try:
        await asyncio.wait([middle_first, 1, middle_last])
        raise AssertionError('expected AttributeError')
    except AttributeError:
        pass
    assert await middle_first == 1
    assert await middle_last == 1

    coro = child()
    try:
        await asyncio.wait([task, coro])
        raise AssertionError('expected TypeError')
    except TypeError:
        pass
    assert await coro == 1

    first_coro = child()
    tail_coro = child()
    try:
        await asyncio.wait([first_coro, task, tail_coro])
        raise AssertionError('expected TypeError')
    except TypeError:
        pass
    assert await first_coro == 1
    assert await tail_coro == 1

    task = None
    coro = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
