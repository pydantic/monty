# /// monty
# cpython-site-packages = true
# ///
import datetime
from decimal import Decimal

import pydantic
from pydantic import PydanticSchemaGenerationError, PydanticUserError, ValidationError

# === The exception classes ===
try:
    raise PydanticSchemaGenerationError('x')
except PydanticUserError as exc:
    assert isinstance(exc, RuntimeError)
    assert isinstance(exc, PydanticSchemaGenerationError)
assert pydantic.VERSION == '2.13.4'

# === Building one from line errors ===
URL = 'https://errors.pydantic.dev/2.13/v/'
exc = ValidationError.from_exception_data(
    'Foo',
    [
        {'type': 'int_parsing', 'loc': ('a', 1), 'input': 'x'},
        {
            'type': 'too_long',
            'loc': ('b.c',),
            'input': [1, 2, 3],
            'ctx': {'field_type': 'List', 'max_length': 2, 'actual_length': 3},
        },
        {'type': 'greater_than', 'loc': (), 'input': 1, 'ctx': {'gt': 5}},
        {'type': 'greater_than', 'input': 1.5, 'ctx': {'gt': 2.5}},
    ],
)
assert isinstance(exc, ValidationError)
assert isinstance(exc, ValueError)
assert exc.title == 'Foo'
assert exc.error_count() == 4
assert str(exc) == (
    '4 validation errors for Foo\n'
    'a.1\n'
    "  Input should be a valid integer, unable to parse string as an integer [type=int_parsing, input_value='x', input_type=str]\n"
    f'    For further information visit {URL}int_parsing\n'
    '`b.c`\n'
    '  List should have at most 2 items after validation, not 3 [type=too_long, input_value=[1, 2, 3], input_type=list]\n'
    f'    For further information visit {URL}too_long\n'
    '  Input should be greater than 5 [type=greater_than, input_value=1, input_type=int]\n'
    f'    For further information visit {URL}greater_than\n'
    '  Input should be greater than 2.5 [type=greater_than, input_value=1.5, input_type=float]\n'
    f'    For further information visit {URL}greater_than'
)
assert repr(exc) == str(exc)
assert exc.errors() == [
    {
        'type': 'int_parsing',
        'loc': ('a', 1),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': 'x',
        'url': f'{URL}int_parsing',
    },
    {
        'type': 'too_long',
        'loc': ('b.c',),
        'msg': 'List should have at most 2 items after validation, not 3',
        'input': [1, 2, 3],
        'ctx': {'field_type': 'List', 'max_length': 2, 'actual_length': 3},
        'url': f'{URL}too_long',
    },
    {
        'type': 'greater_than',
        'loc': (),
        'msg': 'Input should be greater than 5',
        'input': 1,
        'ctx': {'gt': 5},
        'url': f'{URL}greater_than',
    },
    {
        'type': 'greater_than',
        'loc': (),
        'msg': 'Input should be greater than 2.5',
        'input': 1.5,
        'ctx': {'gt': 2.5},
        'url': f'{URL}greater_than',
    },
]
assert exc.errors(include_url=False, include_context=False)[1] == {
    'type': 'too_long',
    'loc': ('b.c',),
    'msg': 'List should have at most 2 items after validation, not 3',
    'input': [1, 2, 3],
}
assert exc.errors(include_input=False)[0] == {
    'type': 'int_parsing',
    'loc': ('a', 1),
    'msg': 'Input should be a valid integer, unable to parse string as an integer',
    'url': f'{URL}int_parsing',
}
assert list(exc.errors()[0]) == ['type', 'loc', 'msg', 'input', 'url']
assert exc.json() == (
    '[{"type":"int_parsing","loc":["a",1],"msg":"Input should be a valid integer, unable to parse string as an integer","input":"x","url":"'
    + URL
    + 'int_parsing"},{"type":"too_long","loc":["b.c"],"msg":"List should have at most 2 items after validation, not 3","input":[1,2,3],"ctx":{"field_type":"List","max_length":2,"actual_length":3},"url":"'
    + URL
    + 'too_long"},{"type":"greater_than","loc":[],"msg":"Input should be greater than 5","input":1,"ctx":{"gt":5},"url":"'
    + URL
    + 'greater_than"},{"type":"greater_than","loc":[],"msg":"Input should be greater than 2.5","input":1.5,"ctx":{"gt":2.5},"url":"'
    + URL
    + 'greater_than"}]'
)
assert exc.json(indent=2, include_url=False, include_input=False, include_context=False) == (
    '[\n'
    '  {\n'
    '    "type": "int_parsing",\n'
    '    "loc": [\n'
    '      "a",\n'
    '      1\n'
    '    ],\n'
    '    "msg": "Input should be a valid integer, unable to parse string as an integer"\n'
    '  },\n'
    '  {\n'
    '    "type": "too_long",\n'
    '    "loc": [\n'
    '      "b.c"\n'
    '    ],\n'
    '    "msg": "List should have at most 2 items after validation, not 3"\n'
    '  },\n'
    '  {\n'
    '    "type": "greater_than",\n'
    '    "loc": [],\n'
    '    "msg": "Input should be greater than 5"\n'
    '  },\n'
    '  {\n'
    '    "type": "greater_than",\n'
    '    "loc": [],\n'
    '    "msg": "Input should be greater than 2.5"\n'
    '  }\n'
    ']'
)

# === Raising and catching ===
try:
    raise exc
except ValueError as caught:
    assert caught is exc
    assert caught.error_count() == 4

# === JSON input mode and hidden inputs ===
exc = ValidationError.from_exception_data('T', [{'type': 'none_required', 'loc': ('x',), 'input': 1}], 'json', True)
assert str(exc) == (
    '1 validation error for T\nx\n  Input should be null [type=none_required]\n'
    f'    For further information visit {URL}none_required'
)
assert exc.errors() == [
    {'type': 'none_required', 'loc': ('x',), 'msg': 'Input should be null', 'input': 1, 'url': f'{URL}none_required'}
]
exc = ValidationError.from_exception_data(title='T', line_errors=[], input_type='python', hide_input=False)
assert exc.error_count() == 0
assert str(exc) == '0 validation errors for T\n'
assert exc.errors() == []
assert exc.json() == '[]'

# === Defaults and odd locations ===
entries = ValidationError.from_exception_data('T', [{'type': 'missing', 'input': {}}]).errors()
assert entries == [{'type': 'missing', 'loc': (), 'msg': 'Field required', 'input': {}, 'url': f'{URL}missing'}]
entries = ValidationError.from_exception_data('T', [{'type': 'missing', 'loc': ()}]).errors()
assert entries[0]['input'] is None
entries = ValidationError.from_exception_data('T', [{'type': 'missing', 'loc': ['a', 1.5], 'input': 1}]).errors()
assert entries[0]['loc'] == ('a', '1.5')

# === Long inputs are truncated in str() only ===
exc = ValidationError.from_exception_data('T', [{'type': 'missing', 'loc': (), 'input': 'é' * 60}])
assert str(exc) == (
    "1 validation error for T\n  Field required [type=missing, input_value='éééééééééééé...ééééééééééé', input_type=str]\n"
    f'    For further information visit {URL}missing'
)
assert exc.errors()[0]['input'] == 'é' * 60

# === Inputs round-trip through errors() and serialise in json() ===
exc = ValidationError.from_exception_data(
    'T',
    [
        {
            'type': 'missing',
            'loc': (),
            'input': {'a': [1, (2, 3)], 'b': frozenset([2]), 'c': b'x', 'd': 1.5, 'e': None, 'f': True, 'g': 10**30},
        }
    ],
)
assert exc.errors()[0]['input'] == {
    'a': [1, (2, 3)],
    'b': frozenset([2]),
    'c': b'x',
    'd': 1.5,
    'e': None,
    'f': True,
    'g': 10**30,
}
assert exc.json(include_url=False) == (
    '[{"type":"missing","loc":[],"msg":"Field required","input":{"a":[1,[2,3]],"b":[2],"c":"x","d":1.5,"e":null,"f":true,"g":1000000000000000000000000000000}}]'
)
temporal = [
    datetime.datetime(2020, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc),
    datetime.datetime(2020, 1, 2, 3, 4, 5, 6, tzinfo=datetime.timezone(datetime.timedelta(hours=-1))),
    datetime.date(2020, 1, 2),
    datetime.time(1, 2),
    datetime.timedelta(seconds=90),
    datetime.timedelta(days=1, seconds=3600, microseconds=500),
    datetime.timedelta(seconds=-1),
    Decimal('1.50'),
    float('nan'),
    float('inf'),
    'é"\n\x01',
]
exc = ValidationError.from_exception_data('T', [{'type': 'missing', 'loc': (), 'input': temporal}])
restored = exc.errors()[0]['input']
assert restored[:8] == temporal[:8]
assert restored[8] != restored[8]
assert restored[9:] == temporal[9:]
assert exc.json(include_url=False) == (
    '[{"type":"missing","loc":[],"msg":"Field required","input":['
    '"2020-01-02T03:04:05Z","2020-01-02T03:04:05.000006-01:00","2020-01-02","01:02:00","PT1M30S","P1DT1H0.0005S","-PT1S",'
    '"1.50",NaN,Infinity,"é\\"\\n\\u0001"]}]'
)

# === Every catalogue type renders ===
for error_type, msg, ctx in [
    ('json_invalid', 'Invalid JSON: eof', {'error': 'eof'}),
    ('json_type', 'JSON input should be string, bytes or bytearray', None),
    ('missing', 'Field required', None),
    ('extra_forbidden', 'Extra inputs are not permitted', None),
    ('dataclass_type', 'Input should be a dictionary or an instance of Foo', {'class_name': 'Foo'}),
    ('less_than_equal', 'Input should be less than or equal to 3', {'le': 3}),
    ('multiple_of', 'Input should be a multiple of 2.5', {'multiple_of': 2.5}),
    ('finite_number', 'Input should be a finite number', None),
    (
        'too_short',
        'Set should have at least 1 item after validation, not 0',
        {'field_type': 'Set', 'min_length': 1, 'actual_length': 0},
    ),
    (
        'too_long',
        'List should have at most 1 item after validation, not more',
        {'field_type': 'List', 'max_length': 1, 'actual_length': None},
    ),
    ('string_too_short', 'String should have at least 2 characters', {'min_length': 2}),
    ('string_too_long', 'String should have at most 1 character', {'max_length': 1}),
    ('string_pattern_mismatch', "String should match pattern '^a'", {'pattern': '^a'}),
    ('literal_error', "Input should be 'a' or 'b'", {'expected': "'a' or 'b'"}),
    (
        'date_parsing',
        'Input should be a valid date in the format YYYY-MM-DD, input is too short',
        {'error': 'input is too short'},
    ),
    ('datetime_type', 'Input should be a valid datetime', None),
    ('time_delta_parsing', 'Input should be a valid timedelta, bad', {'error': 'bad'}),
    ('timezone_offset', 'Timezone offset of 0 required, got 3600', {'tz_expected': 0, 'tz_actual': 3600}),
    ('decimal_max_digits', 'Decimal input should have no more than 2 digits in total', {'max_digits': 2}),
    ('decimal_max_places', 'Decimal input should have no more than 1 decimal place', {'decimal_places': 1}),
    ('bytes_invalid_encoding', 'Data should be valid base64: bad', {'encoding': 'base64', 'encoding_error': 'bad'}),
    (
        'union_tag_invalid',
        "Input tag 'x' found using 'kind' does not match any of the expected tags: 'a', 'b'",
        {'discriminator': "'kind'", 'tag': 'x', 'expected_tags': "'a', 'b'"},
    ),
    ('is_instance_of', 'Input should be an instance of Foo', {'class': 'Foo'}),
    ('value_error', 'Value error, boom', {'error': 'boom'}),
]:
    entry = {'type': error_type, 'loc': (), 'input': 0}
    if ctx is not None:
        entry['ctx'] = ctx
    exc = ValidationError.from_exception_data('T', [entry])
    assert exc.errors()[0]['msg'] == msg
    assert exc.errors()[0]['type'] == error_type
    assert exc.errors()[0].get('ctx') == ctx

# === Mistakes in the line errors ===
try:
    ValidationError.from_exception_data('T', [{'type': 'nope', 'loc': (), 'input': 1}])
    assert False, 'expected KeyError'
except KeyError as exc:
    assert exc.args[0] == "Invalid error type: 'nope'"

try:
    ValidationError.from_exception_data('T', [{'type': 'too_long', 'loc': (), 'input': 1}])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "TooLong: 'field_type' required in context"

try:
    ValidationError.from_exception_data(
        'T',
        [{'type': 'too_long', 'loc': (), 'input': 1, 'ctx': {'field_type': 1, 'max_length': 2, 'actual_length': 3}}],
    )
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "TooLong: 'field_type' context value must be a String"

try:
    ValidationError.from_exception_data('T', [5])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not an instance of 'dict'"

try:
    ValidationError.from_exception_data('T', 5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'line_errors': 'int' object is not an instance of 'list'"

try:
    ValidationError.from_exception_data('T', [], 'xml')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'Invalid error mode: xml'

empty = ValidationError.from_exception_data('T', [])
try:
    empty.json(indent='x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "argument 'indent': 'str' object cannot be interpreted as an integer"
try:
    empty.json(indent=-1)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == "can't convert negative int to unsigned"

# a `ctx` that is present must be a dict, even for a type that needs no context
for ctx, message in [
    (5, "'int' object is not an instance of 'dict'"),
    ('x', "'str' object is not an instance of 'dict'"),
    (None, "'None' is not an instance of 'dict'"),
]:
    try:
        ValidationError.from_exception_data('T', [{'type': 'missing', 'loc': (), 'input': 1, 'ctx': ctx}])
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message

# a numeric constraint takes an int, a float, anything with `__float__` or a str
for gt, msg, reported in [
    (True, 'Input should be greater than 1', 1),
    (10**30, 'Input should be greater than 1000000000000000000000000000000', 10**30),
    (Decimal('1.5'), 'Input should be greater than 1.5', 1.5),
    ('x', 'Input should be greater than x', 'x'),
]:
    entry = ValidationError.from_exception_data(
        'T', [{'type': 'greater_than', 'loc': (), 'input': 1, 'ctx': {'gt': gt}}]
    ).errors()[0]
    assert entry['msg'] == msg
    assert entry.get('ctx') == {'gt': reported}
try:
    ValidationError.from_exception_data('T', [{'type': 'greater_than', 'loc': (), 'input': 1, 'ctx': {'gt': [1]}}])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "GreaterThan: 'gt' context value must be a Number"

# === Non-finite float keys in json() ===
nan_keys = ValidationError.from_exception_data(
    'T', [{'type': 'missing', 'loc': (), 'input': {float('nan'): 1, float('inf'): 2, float('-inf'): 3}}]
)
assert (
    nan_keys.json(include_url=False)
    == '[{"type":"missing","loc":[],"msg":"Field required","input":{"nan":1,"inf":2,"-inf":3}}]'
)
