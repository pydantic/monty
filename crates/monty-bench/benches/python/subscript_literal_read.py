def run():
    """Exercise encoded integer and interned-string keys with adjacent stores."""
    values = [1000, 1001]
    mapping = {'lookup_key': 1001}
    result = 0
    for _ in range(10_000):
        result = values[1]
        result = mapping['lookup_key']
        result = values[1]
        result = mapping['lookup_key']
    return result


assert run() == 1001
