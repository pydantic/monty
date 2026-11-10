def run():
    """Rotate heap results so the destination must acquire and release ownership."""
    values = [[1000], [1001]]
    mapping = {'first': values[0], 'second': values[1]}
    keys = ('first', 'second')
    result = None
    for iteration in range(10_000):
        index = iteration & 1
        key = keys[index]
        result = values[index]
        result = mapping[key]
        result = values[index]
        result = mapping[key]
    assert result is values[1]
    return result


assert run() == [1001]
