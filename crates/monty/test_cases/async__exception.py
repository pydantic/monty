# run-async
# Test that exceptions in async functions propagate correctly


async def raises_error():
    raise ValueError('async error')


await raises_error()  # pyright: ignore
# Raise=ValueError('async error')
