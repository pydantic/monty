async def g():
    return 1


class C:
    x = await g()  # pyright: ignore


# Raise=SyntaxError("'await' outside function")
