# A wait future that completes on its first await must release the reference its
# awaiter owns for the gathering future or adopted task.
import asyncio


async def main():
    async def quick():
        return 'quick'

    done = asyncio.create_task(quick())
    await done

    adopted = asyncio.create_task(asyncio.wait([done]))
    await adopted
    adopted = None

    gathered = asyncio.gather(asyncio.wait([done]))
    await gathered
    gathered = None


asyncio.run(main())
# ref-counts={'asyncio': 1}
