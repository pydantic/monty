# call-external
# run-async
# Awaiting a host module's async tools: the host answers each call with a future.
import asyncio
import tools
from tools import async_call

# === await through the module and the imported name ===
assert await tools.async_call(1) == 1  # pyright: ignore
assert await async_call('x') == 'x'  # pyright: ignore

# === gather runs the calls concurrently ===
results = await asyncio.gather(tools.async_call(1), async_call(2), tools.async_call(3))  # pyright: ignore
assert results == [1, 2, 3]

# === a failing tool raises at the await ===
try:
    await tools.async_fail('ValueError', 'boom')  # pyright: ignore
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'
