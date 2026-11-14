def run():
    """Keep hashing and result ownership stable across repeated native dict hits."""
    values = {'lookup_key': 1001}
    key = 'lookup_key'
    result = 0
    for _ in range(10_000):
        result = values[key]
        result = values[key]
        result = values[key]
        result = values[key]
    return result


assert run() == 1001
