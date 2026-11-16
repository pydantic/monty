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
    (
        '[1 2]',
        "Expecting ',' delimiter: line 1 column 4 (char 3)",
    ),
    (
        '{"a": 1 "b": 2}',
        "Expecting ',' delimiter: line 1 column 9 (char 8)",
    ),
    (
        '[1,',
        'Expecting value: line 1 column 4 (char 3)',
    ),
    (
        '[1,2',
        "Expecting ',' delimiter: line 1 column 5 (char 4)",
    ),
    # positions count characters, not bytes: multibyte input before the error
    (
        '["é", x]',
        'Expecting value: line 1 column 7 (char 6)',
    ),
    (
        '["é" 2]',
        "Expecting ',' delimiter: line 1 column 6 (char 5)",
    ),
    (
        '["日本語",\n "a", x]',
        'Expecting value: line 2 column 7 (char 14)',
    ),
    # heap-backed object key (len >= 2) whose value fails to parse — exercises
    # the DropWithContext path for the just-allocated key (issue #720)
    (
        '{"pe":',
        'Expecting value: line 1 column 7 (char 6)',
    ),
    # input ending inside a container: what was expected next
    ('[', 'Expecting value: line 1 column 2 (char 1)'),
    ('[ ', 'Expecting value: line 1 column 3 (char 2)'),
    ('{', 'Expecting property name enclosed in double quotes: line 1 column 2 (char 1)'),
    ('{ ', 'Expecting property name enclosed in double quotes: line 1 column 3 (char 2)'),
    ('{"a"', "Expecting ':' delimiter: line 1 column 5 (char 4)"),
    ('{"a":1', "Expecting ',' delimiter: line 1 column 7 (char 6)"),
    ('{"a":"b"', "Expecting ',' delimiter: line 1 column 9 (char 8)"),
    ('{"a":1,', 'Expecting property name enclosed in double quotes: line 1 column 8 (char 7)'),
    ('[{"a":1}', "Expecting ',' delimiter: line 1 column 9 (char 8)"),
    ('{"a":[1', "Expecting ',' delimiter: line 1 column 8 (char 7)"),
    ('["{",', 'Expecting value: line 1 column 6 (char 5)'),
    # a malformed literal or number: the longest value the token starts with, then what follows
    ('[tru', 'Expecting value: line 1 column 2 (char 1)'),
    ('tru', 'Expecting value: line 1 column 1 (char 0)'),
    ('{"a": t}', 'Expecting value: line 1 column 7 (char 6)'),
    ('[-', 'Expecting value: line 1 column 2 (char 1)'),
    ('[-Inf', 'Expecting value: line 1 column 2 (char 1)'),
    ('[1.', "Expecting ',' delimiter: line 1 column 3 (char 2)"),
    ('[1e+]', "Expecting ',' delimiter: line 1 column 3 (char 2)"),
    ('[01]', "Expecting ',' delimiter: line 1 column 3 (char 2)"),
    ('[truex]', "Expecting ',' delimiter: line 1 column 6 (char 5)"),
    ('1.5.2', 'Extra data: line 1 column 4 (char 3)'),
    ('["ab"c]', "Expecting ',' delimiter: line 1 column 6 (char 5)"),
    ('[1x]', "Expecting ',' delimiter: line 1 column 3 (char 2)"),
    ('{"k":1, tru', 'Expecting property name enclosed in double quotes: line 1 column 9 (char 8)'),
    # at the depth limit, a malformed first item is reported as such
    ('[' * 201 + ' x', 'Expecting value: line 1 column 203 (char 202)'),
    ('[' * 201 + 'x', 'Expecting value: line 1 column 202 (char 201)'),
    ('{"a":' * 201 + ' x', 'Expecting value: line 1 column 1007 (char 1006)'),
    ('[' * 201 + 'tru', 'Expecting value: line 1 column 202 (char 201)'),
    ('[' * 201, 'Expecting value: line 1 column 202 (char 201)'),
    ('{"a":' * 200 + '{', 'Expecting property name enclosed in double quotes: line 1 column 1002 (char 1001)'),
    ('[' * 200 + '{}', "Expecting ',' delimiter: line 1 column 203 (char 202)"),
]

for source, expected in invalid_cases:
    try:
        json.loads(source)
        assert False, f'invalid JSON should raise JSONDecodeError: {source!r}'
    except json.JSONDecodeError as exc:
        assert str(exc) == expected, f'invalid JSON message mismatch for {source!r}'

# === byte order mark ===
# `bytes` input is decoded as `utf-8-sig`; `str` input rejects the BOM
assert json.loads(b'\xef\xbb\xbf[1]') == [1]
try:
    json.loads(b'\xef\xbb\xbf[1, x]')
    assert False, 'invalid JSON after a BOM should raise JSONDecodeError'
except json.JSONDecodeError as exc:
    assert str(exc) == 'Expecting value: line 1 column 5 (char 4)'
for source in ['\ufeff[1]', '\ufeff']:
    try:
        json.loads(source)
        assert False, 'a str BOM should raise JSONDecodeError'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)'

# === nesting at the depth limit ===
# only a non-empty container beyond 200 levels is refused, so empty nesting parses
nested = json.loads('[' * 201 + ']' * 201)
depth = 0
while nested:
    nested = nested[0]
    depth += 1
assert depth == 200
