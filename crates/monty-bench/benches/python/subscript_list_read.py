def run():
    """Exercise a local subscription and store without arithmetic between reads."""
    values = [1000, 1001]
    key = 1
    result = 0
    for _ in range(10_000):
        result = values[key]
        result = values[key]
        result = values[key]
        result = values[key]
    return result


assert run() == 1001
