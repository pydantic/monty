# `__all__` of bundled modules: CPython's, cut to the names Monty implements.
import annotationlib
import asyncio
import base64
import binascii
import bisect
import collections
import copy
import csv
import dataclasses
import datetime
import decimal
import fnmatch
import functools
import hashlib
import itertools
import json
import math
import os
import os.path
import pathlib
import random
import re
import statistics
import sys
import time
import tomllib
import typing
import unicodedata

is_monty = sys.platform == 'monty'

# === modules whose CPython counterpart defines `__all__` ===
# the names Monty implements: Monty's `__all__` is exactly these, CPython's holds them and more
IMPLEMENTED = [
    ('annotationlib', annotationlib, ['Format', 'ForwardRef', 'get_annotations']),
    ('asyncio', asyncio, ['gather', 'run', 'sleep']),
    (
        'base64',
        base64,
        [
            'a85decode',
            'a85encode',
            'b16decode',
            'b16encode',
            'b32decode',
            'b32encode',
            'b32hexdecode',
            'b32hexencode',
            'b64decode',
            'b64encode',
            'b85decode',
            'b85encode',
            'decodebytes',
            'encodebytes',
            'standard_b64decode',
            'standard_b64encode',
            'urlsafe_b64decode',
            'urlsafe_b64encode',
            'z85decode',
            'z85encode',
        ],
    ),
    ('collections', collections, ['Counter', 'defaultdict', 'deque', 'namedtuple']),
    ('copy', copy, ['copy', 'deepcopy']),
    (
        'csv',
        csv,
        [
            'Dialect',
            'DictReader',
            'DictWriter',
            'Error',
            'QUOTE_ALL',
            'QUOTE_MINIMAL',
            'QUOTE_NONE',
            'QUOTE_NONNUMERIC',
            'QUOTE_NOTNULL',
            'QUOTE_STRINGS',
            'excel',
            'excel_tab',
            'field_size_limit',
            'get_dialect',
            'list_dialects',
            'reader',
            'unix_dialect',
            'writer',
        ],
    ),
    ('dataclasses', dataclasses, ['FrozenInstanceError', 'MISSING', 'dataclass', 'field', 'is_dataclass']),
    ('datetime', datetime, ['date', 'datetime', 'time', 'timedelta', 'timezone']),
    ('fnmatch', fnmatch, ['filter', 'fnmatch', 'fnmatchcase', 'translate']),
    ('functools', functools, ['cache', 'lru_cache', 'partial', 'reduce']),
    (
        'hashlib',
        hashlib,
        [
            'algorithms_available',
            'algorithms_guaranteed',
            'blake2b',
            'blake2s',
            'md5',
            'new',
            'pbkdf2_hmac',
            'sha1',
            'sha224',
            'sha256',
            'sha384',
            'sha3_224',
            'sha3_256',
            'sha3_384',
            'sha3_512',
            'sha512',
            'shake_128',
            'shake_256',
        ],
    ),
    ('json', json, ['JSONDecodeError', 'dump', 'dumps', 'load', 'loads']),
    (
        'os',
        os,
        [
            'altsep',
            'chdir',
            'curdir',
            'defpath',
            'devnull',
            'environ',
            'extsep',
            'fspath',
            'getcwd',
            'getcwdb',
            'getenv',
            'linesep',
            'listdir',
            'makedirs',
            'mkdir',
            'name',
            'pardir',
            'path',
            'pathsep',
            'remove',
            'rename',
            'replace',
            'rmdir',
            'scandir',
            'sep',
            'stat',
            'unlink',
            'urandom',
            'walk',
        ],
    ),
    (
        'os.path',
        os.path,
        [
            'abspath',
            'altsep',
            'basename',
            'commonpath',
            'commonprefix',
            'curdir',
            'defpath',
            'devnull',
            'dirname',
            'exists',
            'expanduser',
            'expandvars',
            'extsep',
            'getatime',
            'getctime',
            'getmtime',
            'getsize',
            'isabs',
            'isdevdrive',
            'isdir',
            'isfile',
            'isjunction',
            'islink',
            'ismount',
            'join',
            'lexists',
            'normcase',
            'normpath',
            'pardir',
            'pathsep',
            'realpath',
            'relpath',
            'samefile',
            'samestat',
            'sep',
            'split',
            'splitdrive',
            'splitext',
            'splitroot',
            'supports_unicode_filenames',
        ],
    ),
    ('pathlib', pathlib, ['Path']),
    (
        'random',
        random,
        [
            'Random',
            'betavariate',
            'binomialvariate',
            'choice',
            'choices',
            'expovariate',
            'gammavariate',
            'gauss',
            'getrandbits',
            'getstate',
            'lognormvariate',
            'normalvariate',
            'paretovariate',
            'randbytes',
            'randint',
            'random',
            'randrange',
            'sample',
            'seed',
            'setstate',
            'shuffle',
            'triangular',
            'uniform',
            'vonmisesvariate',
            'weibullvariate',
        ],
    ),
    (
        're',
        re,
        [
            'A',
            'ASCII',
            'DOTALL',
            'I',
            'IGNORECASE',
            'M',
            'MULTILINE',
            'Match',
            'NOFLAG',
            'Pattern',
            'PatternError',
            'S',
            'compile',
            'error',
            'escape',
            'findall',
            'finditer',
            'fullmatch',
            'match',
            'search',
            'split',
            'sub',
        ],
    ),
    (
        'statistics',
        statistics,
        [
            'StatisticsError',
            'fmean',
            'mean',
            'median',
            'median_high',
            'median_low',
            'mode',
            'multimode',
            'pstdev',
            'pvariance',
            'quantiles',
            'stdev',
            'variance',
        ],
    ),
    ('tomllib', tomllib, ['TOMLDecodeError', 'load', 'loads']),
    (
        'typing',
        typing,
        [
            'Annotated',
            'Any',
            'Callable',
            'ClassVar',
            'Dict',
            'Final',
            'ForwardRef',
            'FrozenSet',
            'Generator',
            'Generic',
            'Iterable',
            'Iterator',
            'List',
            'Literal',
            'Mapping',
            'Never',
            'NoReturn',
            'Optional',
            'Protocol',
            'Self',
            'Sequence',
            'Set',
            'TYPE_CHECKING',
            'Tuple',
            'Type',
            'TypeVar',
            'Union',
            'get_type_hints',
        ],
    ),
]
for name, module, names in IMPLEMENTED:
    if is_monty:
        assert (name, sorted(module.__all__)) == (name, names)
    else:
        assert (name, [n for n in names if n not in module.__all__]) == (name, [])
    # every name resolves; reading `os.environ` needs the host
    if name != 'os':
        ns = {}
        exec('from ' + name + ' import *', ns)
        assert (name, sorted(k for k in ns if k != '__builtins__')) == (name, sorted(module.__all__))

# === modules whose CPython counterpart defines none ===
for name, module in [
    ('binascii', binascii),
    ('bisect', bisect),
    ('decimal', decimal),
    ('itertools', itertools),
    ('math', math),
    ('sys', sys),
    ('time', time),
    ('unicodedata', unicodedata),
]:
    assert not hasattr(module, '__all__'), name


# === public names outside `__all__` are not star-imported ===
from base64 import *

assert base64.MAXBINSIZE == 57
try:
    MAXBINSIZE
    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'MAXBINSIZE' is not defined"
