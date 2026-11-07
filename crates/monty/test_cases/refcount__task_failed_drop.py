# Dropping a failed task handle must release its coroutine and cached error.
import asyncio


async def fails():
    raise ValueError('bad')


async def main():
    failed = asyncio.create_task(fails())
    try:
        await failed
    except ValueError:
        pass
    assert failed.done()
    assert repr(failed.exception()) == "ValueError('bad')"
    failed = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
