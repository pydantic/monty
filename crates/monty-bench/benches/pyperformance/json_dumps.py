# Adapted from pyperformance bm_json_dumps at ccc0aeb7ad46d65b6dcd4160e0fdda4d885852dd.
# See README.md for harness changes and LICENSE for upstream terms.
import json

EMPTY = ({}, 2000)
SIMPLE_DATA = {'key1': 0, 'key2': True, 'key3': 'value', 'key4': 'foo', 'key5': 'string'}
SIMPLE = (SIMPLE_DATA, 1000)
NESTED_DATA = {
    'key1': 0,
    'key2': SIMPLE[0],
    'key3': 'value',
    'key4': SIMPLE[0],
    'key5': SIMPLE[0],
    'key': '\u0105\u0107\u017c',
}
NESTED = (NESTED_DATA, 1000)
HUGE = ([NESTED[0]] * 1000, 1)

CASES = ['EMPTY', 'SIMPLE', 'NESTED', 'HUGE']


def bench_json_dumps(data):
    for obj, count_it in data:
        for _ in count_it:
            json.dumps(obj)


bench_json_dumps([(obj, range(count)) for obj, count in [EMPTY, SIMPLE, NESTED, HUGE]])
len(json.dumps(NESTED_DATA))
