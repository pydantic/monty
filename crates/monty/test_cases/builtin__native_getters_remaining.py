from dataclasses import dataclass, field
from decimal import Decimal
from functools import partial
from pathlib import Path
from typing import ForwardRef, Union
import hashlib
import io
import random

decimal_value = Decimal('1.25')
assert Decimal.real.__get__(decimal_value) is decimal_value
assert Decimal.imag.__get__(decimal_value) == 0

for stream in [io.StringIO(), io.BytesIO()]:
    closed = type(stream).closed
    assert not closed.__get__(stream)
    stream.close()
    assert closed.__get__(stream)
assert io.StringIO.encoding.__get__(io.StringIO()) is None

for hash_obj in [hashlib.sha256(), hashlib.shake_128(), hashlib.blake2b(), hashlib.blake2s()]:
    hash_type = type(hash_obj)
    assert hash_type.name.__get__(hash_obj) == hash_obj.name
    assert hash_type.digest_size.__get__(hash_obj) == hash_obj.digest_size
    assert hash_type.block_size.__get__(hash_obj) == hash_obj.block_size
assert hashlib.blake2b().MAX_DIGEST_SIZE == hashlib.blake2b.MAX_DIGEST_SIZE

path = Path('/tmp/report.tar.gz')
assert type(path).name.__get__(path) == 'report.tar.gz'
assert type(path).parent.__get__(path) == Path('/tmp')
assert type(path).suffixes.__get__(path) == ['.tar', '.gz']


def func(x, y=0):
    return x + y


bound = partial(func, 1, y=2)
assert type(bound).func.__get__(bound) is func
assert type(bound).args.__get__(bound) == (1,)
assert type(bound).keywords.__get__(bound) == {'y': 2}
assert bound.func(3, y=4) == 7
assert random.Random.VERSION == random.Random().VERSION == 3

alias = list[int]
assert type(alias).__origin__.__get__(alias) is list
assert type(alias).__args__.__get__(alias) == (int,)
assert type(alias).__parameters__.__get__(alias) == ()
union = int | str
assert Union.__args__.__get__(union) == (int, str)
assert Union.__origin__.__get__(union) is Union
assert ForwardRef.__forward_arg__.__get__(ForwardRef('Later')) == 'Later'

f = field(default=4)
assert type(f).default.__get__(f) == 4


@dataclass(frozen=True)
class Example:
    x: int


params = Example.__dataclass_params__
assert type(params).frozen.__get__(params) is True

error = ValueError('bad value')
assert BaseException.args.__get__(error) == ('bad value',)
assert BaseException.__cause__.__get__(error) is None
assert StopIteration.value.__get__(StopIteration(42)) == 42

wrapper = classmethod(func)
assert classmethod.__func__.__get__(wrapper) is func
assert classmethod.__wrapped__.__get__(wrapper) is func
prop = property(func)
assert property.fget.__get__(prop) is func
