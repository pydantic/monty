# xfail=cpython
# Tests for Monty-specific sys module values

import sys

# === sys.version ===
assert sys.version == '3.14.0 (Monty)', f'version should be 3.14.0 (Monty), got {sys.version!r}'

# === sys.version_info exact values ===
assert sys.version_info[0] == 3
assert sys.version_info[1] == 14
assert sys.version_info[2] == 0
assert sys.version_info[3] == 'final'
assert sys.version_info[4] == 0

# === sys.version_info named attributes ===
assert sys.version_info.major == 3
assert sys.version_info.minor == 14
assert sys.version_info.micro == 0
assert sys.version_info.releaselevel == 'final'
assert sys.version_info.serial == 0

# === sys.version_info tuple equality ===
# This works because NamedTuple equality compares only by elements, not type_name
assert sys.version_info == (3, 14, 0, 'final', 0)

# === sys.platform ===
assert sys.platform == 'monty', f'platform should be monty, got {sys.platform!r}'

# === sys.hexversion ===
assert sys.hexversion == 0x030E00F0

# === sys.copyright ===
assert sys.copyright == 'Copyright (c) Pydantic Services Inc. 2026 to present'

# === The sandbox has no install tree ===
assert sys.executable == ''
assert sys.prefix == ''
assert sys.exec_prefix == ''
assert sys.base_prefix == ''
assert sys.base_exec_prefix == ''
# prefix == base_prefix, so the usual "am I in a virtualenv?" test says no
assert sys.prefix == sys.base_prefix
assert sys.platlibdir == 'lib'
assert sys.abiflags == ''

# === Monty never writes bytecode ===
assert sys.dont_write_bytecode is True
assert sys.pycache_prefix is None

# === sys.builtin_module_names is the whole importable set ===
# `gc` is only registered in test builds, so compare against the production set.
assert tuple(name for name in sys.builtin_module_names if name != 'gc') == (
    'asyncio',
    'base64',
    'binascii',
    'collections',
    'dataclasses',
    'datetime',
    'functools',
    'itertools',
    'json',
    'math',
    'os',
    'pathlib',
    're',
    'sys',
    'typing',
    'unicodedata',
)

# === Attributes describing CPython internals stay absent ===
for missing in ('flags', 'hash_info', 'int_info', 'thread_info', 'ps1', 'ps2'):
    try:
        getattr(sys, missing)
        assert False, f'expected sys.{missing} to raise AttributeError'
    except AttributeError as exc:
        assert str(exc) == f"'module' object has no attribute '{missing}'"
