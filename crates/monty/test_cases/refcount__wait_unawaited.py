# Dropping a wait future before its first await must release the owned `fs`
# iterable and every task it holds.
import asyncio


async def child():
    return 1


async def main():
    task = asyncio.create_task(child())
    await task
    return_when = ['not validated until await']
    waiting = asyncio.wait([task], return_when=return_when)
    waiting = None
    return_when = None
    task = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
