import json

# === invalid JSON messages ===
invalid_cases = [
    (
        "{'a': 1}",
        'Expecting property name enclosed in double quotes: line 1 column 2 (char 1)',
    ),
    (
        '{"a": 1,}',
        'Illegal trailing comma before end of object: line 1 column 8 (char 7)',
    ),
    (
        '[1,]',
        'Illegal trailing comma before end of array: line 1 column 3 (char 2)',
    ),
    (
        '"abc',
        'Unterminated string starting at: line 1 column 1 (char 0)',
    ),
    (
        '',
        'Expecting value: line 1 column 1 (char 0)',
    ),
    (
        'true false',
        'Extra data: line 1 column 6 (char 5)',
    ),
    (
        '1\n2',
        'Extra data: line 2 column 1 (char 2)',
    ),
    (
        '[1]\n{"a": 2}',
        'Extra data: line 2 column 1 (char 4)',
    ),
    (
        '[,1]',
        'Expecting value: line 1 column 2 (char 1)',
    ),
    (
        '{"a" 1}',
        "Expecting ':' delimiter: line 1 column 6 (char 5)",
    ),
    (
        '"\\x"',
        'Invalid \\escape: line 1 column 2 (char 1)',
    ),
    (
        '"\\u12X4"',
        'Invalid \\uXXXX escape: line 1 column 3 (char 2)',
    ),
    (
        '[1',
        "Expecting ',' delimiter: line 1 column 3 (char 2)",
    ),
    (
        '{"a": 1',
        "Expecting ',' delimiter: line 1 column 8 (char 7)",
    ),
    (
        '{"a": [1, 2,]}',
        'Illegal trailing comma before end of array: line 1 column 12 (char 11)',
    ),
    (
        '{"a": {"b": 1,}}',
        'Illegal trailing comma before end of object: line 1 column 14 (char 13)',
    ),
    (
        '[\n  1,\n]',
        'Illegal trailing comma before end of array: line 2 column 4 (char 5)',
    ),
    (
        '{\n  "a": 1,\n}',
        'Illegal trailing comma before end of object: line 2 column 9 (char 10)',
    ),
    (
        'True',
        'Expecting value: line 1 column 1 (char 0)',
    ),
]

for source, expected in invalid_cases:
    try:
        json.loads(source)
        assert False, f'invalid JSON should raise JSONDecodeError: {source!r}'
    except json.JSONDecodeError as exc:
        assert str(exc) == expected, f'invalid JSON message mismatch for {source!r}'
