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
]

for source, expected in invalid_cases:
    try:
        json.loads(source)
        assert False, f'invalid JSON should raise JSONDecodeError: {source!r}'
    except json.JSONDecodeError as exc:
        assert str(exc) == expected, f'invalid JSON message mismatch for {source!r}'
