# /// monty
# cpython-site-packages = true
# ///
from decimal import Decimal
from typing import Any

from pydantic import TypeAdapter, ValidationError


def errors(adapter, value, **kwargs):
    try:
        adapter.validate_python(value, **kwargs)
    except ValidationError as exc:
        return exc.errors(include_url=False)
    assert False, 'expected a ValidationError'


# === int ===
Int = TypeAdapter(int)
assert Int.validate_python(1) == 1
assert Int.validate_python(True) == 1
assert Int.validate_python('1') == 1
assert Int.validate_python(' 12 ') == 12
assert Int.validate_python('1_000') == 1000
assert Int.validate_python('+5') == 5
assert Int.validate_python('-0') == 0
assert Int.validate_python('007') == 7
assert Int.validate_python('1.0') == 1
assert Int.validate_python(1.0) == 1
assert Int.validate_python(b'7') == 7
assert Int.validate_python(Decimal('3')) == 3
assert Int.validate_python(10**30) == 10**30
assert Int.validate_python('123456789012345678901234567890') == 123456789012345678901234567890
assert Int.validate_python('1', strict=False) == 1
assert type(Int.validate_python(True)) is int
assert errors(Int, 2.5) == [
    {
        'type': 'int_from_float',
        'loc': (),
        'msg': 'Input should be a valid integer, got a number with a fractional part',
        'input': 2.5,
    }
]
assert errors(Int, '1.5') == [
    {
        'type': 'int_parsing',
        'loc': (),
        'msg': 'Input should be a valid integer, unable to parse string as an integer',
        'input': '1.5',
    }
]
assert errors(Int, 'x')[0]['type'] == 'int_parsing'
assert errors(Int, '0x1')[0]['type'] == 'int_parsing'
assert errors(Int, '')[0]['type'] == 'int_parsing'
assert errors(Int, '1__0')[0]['type'] == 'int_parsing'
assert errors(Int, '_1')[0]['type'] == 'int_parsing'
assert errors(Int, '+-1')[0]['type'] == 'int_parsing'
assert errors(Int, Decimal('3.5'))[0]['type'] == 'int_from_float'
assert errors(Int, Decimal('inf'))[0]['type'] == 'finite_number'
assert errors(Int, float('nan'))[0]['type'] == 'finite_number'
assert errors(Int, float('inf'))[0]['type'] == 'finite_number'
assert errors(Int, 1e30)[0]['type'] == 'int_parsing_size'
assert errors(Int, None) == [{'type': 'int_type', 'loc': (), 'msg': 'Input should be a valid integer', 'input': None}]
assert errors(Int, [1])[0]['type'] == 'int_type'
assert errors(Int, b'\xff')[0]['type'] == 'int_parsing'
assert errors(Int, '1', strict=True)[0]['type'] == 'int_type'
assert errors(Int, True, strict=True)[0]['type'] == 'int_type'
assert errors(Int, 1.0, strict=True)[0]['type'] == 'int_type'
assert Int.validate_python(5, strict=True) == 5
assert Int.validate_python(10**30, strict=True) == 10**30

# === float ===
Float = TypeAdapter(float)
assert Float.validate_python(1.5) == 1.5
assert Float.validate_python(1) == 1.0
assert type(Float.validate_python(1)) is float
assert Float.validate_python(True) == 1.0
assert Float.validate_python(100000000000000000000000) == 1e23
assert Float.validate_python(10**23) == 1e23
assert Float.validate_python('1.5') == 1.5
assert Float.validate_python(' 2 ') == 2.0
assert Float.validate_python('1_5.5') == 15.5
assert Float.validate_python(b'2.5') == 2.5
assert Float.validate_python('1e3') == 1000.0
assert Float.validate_python('inf') == float('inf')
assert Float.validate_python('-inf') == float('-inf')
nan = Float.validate_python('nan')
assert nan != nan
assert Float.validate_python(float('inf')) == float('inf')
assert Float.validate_python(10**30) == 1e30
assert errors(Float, 'x') == [
    {
        'type': 'float_parsing',
        'loc': (),
        'msg': 'Input should be a valid number, unable to parse string as a number',
        'input': 'x',
    }
]
assert errors(Float, None) == [
    {'type': 'float_type', 'loc': (), 'msg': 'Input should be a valid number', 'input': None}
]
assert errors(Float, [1.5])[0]['type'] == 'float_type'
assert errors(Float, '1.5', strict=True)[0]['type'] == 'float_type'
assert errors(Float, True, strict=True)[0]['type'] == 'float_type'
assert Float.validate_python(1, strict=True) == 1.0
assert Float.validate_python(1.5, strict=True) == 1.5

# === str ===
Str = TypeAdapter(str)
assert Str.validate_python('a') == 'a'
assert Str.validate_python(b'bytes') == 'bytes'
assert Str.validate_python('') == ''
assert errors(Str, 1) == [{'type': 'string_type', 'loc': (), 'msg': 'Input should be a valid string', 'input': 1}]
assert errors(Str, None)[0]['type'] == 'string_type'
assert errors(Str, ['a'])[0]['type'] == 'string_type'
assert errors(Str, b'\xff') == [
    {
        'type': 'string_unicode',
        'loc': (),
        'msg': 'Input should be a valid string, unable to parse raw data as a unicode string',
        'input': b'\xff',
    }
]
assert errors(Str, b'bytes', strict=True)[0]['type'] == 'string_type'
assert Str.validate_python('a', strict=True) == 'a'

# === bool ===
Bool = TypeAdapter(bool)
assert Bool.validate_python(True) is True
assert Bool.validate_python(False) is False
for truthy in ['yes', 'YES', 'y', 'on', 'true', 'True', 't', '1', 1, 1.0, b'true']:
    assert Bool.validate_python(truthy) is True
for falsy in ['no', 'N', 'off', 'false', 'FALSE', 'f', '0', 0, 0.0, b'0']:
    assert Bool.validate_python(falsy) is False
assert errors(Bool, 'maybe') == [
    {
        'type': 'bool_parsing',
        'loc': (),
        'msg': 'Input should be a valid boolean, unable to interpret input',
        'input': 'maybe',
    }
]
assert errors(Bool, 2)[0]['type'] == 'bool_parsing'
assert errors(Bool, 1.5)[0]['type'] == 'bool_type'
assert errors(Bool, None) == [{'type': 'bool_type', 'loc': (), 'msg': 'Input should be a valid boolean', 'input': None}]
assert errors(Bool, [])[0]['type'] == 'bool_type'
assert errors(Bool, 1, strict=True)[0]['type'] == 'bool_type'
assert errors(Bool, 'yes', strict=True)[0]['type'] == 'bool_type'
assert Bool.validate_python(True, strict=True) is True

# === bytes ===
Bytes = TypeAdapter(bytes)
assert Bytes.validate_python(b'abc') == b'abc'
assert Bytes.validate_python('abc') == b'abc'
assert Bytes.validate_python('é') == b'\xc3\xa9'
assert errors(Bytes, 1) == [{'type': 'bytes_type', 'loc': (), 'msg': 'Input should be a valid bytes', 'input': 1}]
assert errors(Bytes, None)[0]['type'] == 'bytes_type'
assert errors(Bytes, 'abc', strict=True)[0]['type'] == 'bytes_type'

# === None and Any ===
NoneAdapter = TypeAdapter(None)
assert NoneAdapter.validate_python(None) is None
assert errors(NoneAdapter, 1) == [{'type': 'none_required', 'loc': (), 'msg': 'Input should be None', 'input': 1}]
assert errors(NoneAdapter, 0)[0]['type'] == 'none_required'
assert TypeAdapter(type(None)).validate_python(None) is None

AnyAdapter = TypeAdapter(Any)
for value in [1, 'a', None, [1, 'a'], {'k': (1,)}]:
    assert AnyAdapter.validate_python(value) == value
shared = [1, 2]
assert AnyAdapter.validate_python(shared) is shared

# === Decimal ===
DecimalAdapter = TypeAdapter(Decimal)
assert DecimalAdapter.validate_python(Decimal('1.5')) == Decimal('1.5')
assert DecimalAdapter.validate_python('1.5') == Decimal('1.5')
assert DecimalAdapter.validate_python('1.50') == Decimal('1.50')
assert str(DecimalAdapter.validate_python('1.50')) == '1.50'
assert DecimalAdapter.validate_python(1) == Decimal('1')
assert DecimalAdapter.validate_python(1.5) == Decimal('1.5')
assert DecimalAdapter.validate_python(0.1) == Decimal('0.1')
assert DecimalAdapter.validate_python(10**30) == Decimal(10**30)
assert DecimalAdapter.validate_python(1000000000000000000000000000000) == Decimal(10**30)
assert DecimalAdapter.validate_python((0, (1, 5), -1)) == Decimal('1.5')
assert errors(DecimalAdapter, 'inf') == [
    {'type': 'finite_number', 'loc': (), 'msg': 'Input should be a finite number', 'input': 'inf'}
]
assert errors(DecimalAdapter, Decimal('nan'))[0]['type'] == 'finite_number'
assert errors(DecimalAdapter, 'x') == [
    {'type': 'decimal_parsing', 'loc': (), 'msg': 'Input should be a valid decimal', 'input': 'x'}
]
assert errors(DecimalAdapter, True) == [
    {
        'type': 'decimal_type',
        'loc': (),
        'msg': 'Decimal input should be an integer, float, string or Decimal object',
        'input': True,
    }
]
assert errors(DecimalAdapter, [1])[0]['type'] == 'decimal_type'
assert errors(DecimalAdapter, None)[0]['type'] == 'decimal_type'
assert errors(DecimalAdapter, (1, 2))[0]['type'] == 'decimal_type'
assert errors(DecimalAdapter, '1', strict=True) == [
    {
        'type': 'is_instance_of',
        'loc': (),
        'msg': 'Input should be an instance of Decimal',
        'input': '1',
        'ctx': {'class': 'Decimal'},
    }
]
assert DecimalAdapter.validate_python(Decimal('2'), strict=True) == Decimal('2')

# === The validated value is a new object for a coerced input only ===
text = 'abc'
assert Str.validate_python(text) is text
number = 10**40
assert Int.validate_python(number) is number
