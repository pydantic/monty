VALUES = [1000, 1001]
MAPPING = {'lookup_key': 1001}
KEY = 'lookup_key'


def run():
    """Include original key-load continuations in global subscription accounting."""
    result = 0
    for _ in range(10_000):
        result = VALUES[1]
        result = MAPPING[KEY]
        result = MAPPING['lookup_key']
        result = VALUES[1]
    return result


assert run() == 1001
