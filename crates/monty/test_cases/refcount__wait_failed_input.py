# wait() over a failed task returns it in `done`; the wait and WaitSlot must
# release it without losing or double-freeing the cached exception.
import asyncio


async def fails():
    raise ValueError('bad')


async def main():
    failed = asyncio.create_task(fails())
    done, pending = await asyncio.wait([failed], return_when=asyncio.FIRST_EXCEPTION)
    assert done == {failed}
    assert pending == set()
    assert repr(failed.exception()) == "ValueError('bad')"
    failed = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
