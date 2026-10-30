async def g():
    return 1


def f():
    return await g()  # pyright: ignore


# Raise=SyntaxError("'await' outside async function")
