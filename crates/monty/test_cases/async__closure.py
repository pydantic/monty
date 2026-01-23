# run-async
# Async function capturing variables from enclosing scope


def make_adder(n):
    async def adder(x):
        return x + n

    return adder


add_five = make_adder(5)
result = await add_five(10)  # pyright: ignore
assert result == 15, 'async closure should capture variables'
