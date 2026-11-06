import fnmatch

assert fnmatch.fnmatchcase('x', '[!x]') is False
assert fnmatch.fnmatchcase('z', '[!x]') is True
assert fnmatch.fnmatchcase('[!]', '[!]') is True
assert fnmatch.fnmatchcase('x', '[!]') is False
assert fnmatch.fnmatchcase('', '[!]') is False
assert fnmatch.fnmatchcase('x', '[b-a]') is False
assert fnmatch.fnmatchcase('z', '[b-a]') is False
assert fnmatch.fnmatchcase('x', '[!b-a!x]') is False
assert fnmatch.fnmatchcase('z', '[!b-a!x]') is True
assert fnmatch.fnmatchcase('!', '[!b-a!x]') is False

pattern = '[b-a!x]'
names = ['x', 'z', '!']
byte_names = [b'x', b'z', b'!']
empty_pattern_names = ['x', 'z', '!', '']
byte_empty_pattern_names = [b'x', b'z', b'!', b'']
boundary_names = ['-', 'x', 'z', '!', '']
byte_boundary_names = [b'-', b'x', b'z', b'!', b'']

observed = {
    'translate': fnmatch.translate(pattern),
    'fnmatchcase': {name: fnmatch.fnmatchcase(name, pattern) for name in names},
    'fnmatch': {name: fnmatch.fnmatch(name, pattern) for name in names},
    'filter': fnmatch.filter(names, pattern),
    'bytes_fnmatchcase': {name: fnmatch.fnmatchcase(name, b'[b-a!x]') for name in byte_names},
    'bytes_fnmatch': {name: fnmatch.fnmatch(name, b'[b-a!x]') for name in byte_names},
    'bytes_filter': fnmatch.filter(byte_names, b'[b-a!x]'),
    'exposed_empty': {
        'translate': fnmatch.translate('[b-a!]'),
        'fnmatchcase': {name: fnmatch.fnmatchcase(name, '[b-a!]') for name in empty_pattern_names},
        'fnmatch': {name: fnmatch.fnmatch(name, '[b-a!]') for name in empty_pattern_names},
        'filter': fnmatch.filter(empty_pattern_names, '[b-a!]'),
    },
    'bytes_exposed_empty': {
        'fnmatchcase': {name: fnmatch.fnmatchcase(name, b'[b-a!]') for name in byte_empty_pattern_names},
        'fnmatch': {name: fnmatch.fnmatch(name, b'[b-a!]') for name in byte_empty_pattern_names},
        'filter': fnmatch.filter(byte_empty_pattern_names, b'[b-a!]'),
    },
    'first_boundary': {
        'translate': fnmatch.translate('[b-a!-x]'),
        'fnmatchcase': {name: fnmatch.fnmatchcase(name, '[b-a!-x]') for name in boundary_names},
        'fnmatch': {name: fnmatch.fnmatch(name, '[b-a!-x]') for name in boundary_names},
        'filter': fnmatch.filter(boundary_names, '[b-a!-x]'),
    },
    'bytes_first_boundary': {
        'fnmatchcase': {name: fnmatch.fnmatchcase(name, b'[b-a!-x]') for name in byte_boundary_names},
        'fnmatch': {name: fnmatch.fnmatch(name, b'[b-a!-x]') for name in byte_boundary_names},
        'filter': fnmatch.filter(byte_boundary_names, b'[b-a!-x]'),
    },
}

expected = {
    'translate': r'(?s:[^x])\z',
    'fnmatchcase': {'x': False, 'z': True, '!': True},
    'fnmatch': {'x': False, 'z': True, '!': True},
    'filter': ['z', '!'],
    'bytes_fnmatchcase': {b'x': False, b'z': True, b'!': True},
    'bytes_fnmatch': {b'x': False, b'z': True, b'!': True},
    'bytes_filter': [b'z', b'!'],
    'exposed_empty': {
        'translate': r'(?s:.)\z',
        'fnmatchcase': {'x': True, 'z': True, '!': True, '': False},
        'fnmatch': {'x': True, 'z': True, '!': True, '': False},
        'filter': ['x', 'z', '!'],
    },
    'bytes_exposed_empty': {
        'fnmatchcase': {b'x': True, b'z': True, b'!': True, b'': False},
        'fnmatch': {b'x': True, b'z': True, b'!': True, b'': False},
        'filter': [b'x', b'z', b'!'],
    },
    'first_boundary': {
        'translate': r'(?s:[^-x])\z',
        'fnmatchcase': {'-': False, 'x': False, 'z': True, '!': True, '': False},
        'fnmatch': {'-': False, 'x': False, 'z': True, '!': True, '': False},
        'filter': ['z', '!'],
    },
    'bytes_first_boundary': {
        'fnmatchcase': {b'-': False, b'x': False, b'z': True, b'!': True, b'': False},
        'fnmatch': {b'-': False, b'x': False, b'z': True, b'!': True, b'': False},
        'filter': [b'z', b'!'],
    },
}

assert observed == expected, observed
