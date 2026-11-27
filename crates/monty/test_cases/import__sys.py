# Tests for sys module import

import sys

# === sys.version ===
# Check that version is a non-empty string (exact value differs between interpreters)
assert isinstance(sys.version, str)
assert len(sys.version) > 0

# === sys.version_info ===
# Test index access returns integers for first 3 elements
assert isinstance(sys.version_info[0], int)
assert isinstance(sys.version_info[1], int)
assert isinstance(sys.version_info[2], int)
assert isinstance(sys.version_info[3], str)
assert isinstance(sys.version_info[4], int)

# Test negative indexing
assert sys.version_info[-1] == sys.version_info[4]
assert sys.version_info[-2] == sys.version_info[3]
assert sys.version_info[-5] == sys.version_info[0]

# Test named attribute access matches index access
assert sys.version_info.major == sys.version_info[0]
assert sys.version_info.minor == sys.version_info[1]
assert sys.version_info.micro == sys.version_info[2]
assert sys.version_info.releaselevel == sys.version_info[3]
assert sys.version_info.serial == sys.version_info[4]

# Test len
assert len(sys.version_info) == 5

# Test tuple equality (works after fixing NamedTuple equality)
v = sys.version_info
assert (v[0], v[1]) == (v.major, v.minor)
assert v.major == v[0]
assert v.minor == v[1]

# === sys.platform ===
# Check that platform is a non-empty string (exact value differs between interpreters)
assert isinstance(sys.platform, str)
assert len(sys.platform) > 0

# === sys.stdout and sys.stderr ===
# These should exist - we test by accessing them (will fail if not present)
stdout = sys.stdout
stderr = sys.stderr

# === sys.maxsize / sys.maxunicode ===
assert sys.maxsize == 9223372036854775807
assert sys.maxunicode == 1114111
assert len(chr(sys.maxunicode)) == 1

# === sys.byteorder ===
assert sys.byteorder == 'little'

# === sys.hexversion ===
# hexversion packs version_info as major<<24 | minor<<16 | micro<<8 | level<<4 | serial
assert sys.hexversion >> 24 == sys.version_info.major
assert (sys.hexversion >> 16) & 0xFF == sys.version_info.minor
assert (sys.hexversion >> 8) & 0xFF == sys.version_info.micro

# === sys.api_version ===
assert sys.api_version == 1013

# === sys.float_repr_style ===
assert sys.float_repr_style == 'short'

# === sys.float_info ===
# Fixed by IEEE 754 binary64, so identical on both interpreters
assert sys.float_info.max == 1.7976931348623157e308
assert sys.float_info.max_exp == 1024
assert sys.float_info.max_10_exp == 308
assert sys.float_info.min == 2.2250738585072014e-308
assert sys.float_info.min_exp == -1021
assert sys.float_info.min_10_exp == -307
assert sys.float_info.dig == 15
assert sys.float_info.mant_dig == 53
assert sys.float_info.epsilon == 2.220446049250313e-16
assert sys.float_info.radix == 2
assert sys.float_info.rounds == 1
assert len(sys.float_info) == 11
assert sys.float_info[0] == sys.float_info.max
assert sys.float_info[-1] == sys.float_info.rounds
assert repr(sys.float_info).startswith('sys.float_info(max=')

# === sys.hash_info ===
hi = sys.hash_info
assert isinstance(hi, tuple)
assert len(hi) == 9
assert len(tuple(hi)) == 9

# Named attribute access
assert hi.width == 64
assert hi.modulus == 2305843009213693951
assert hi.inf == 314159
assert hi.nan == 0
assert hi.imag == 1000003
assert hi.algorithm == 'siphash13'
assert hi.hash_bits == 64
assert hi.seed_bits == 128
assert hi.cutoff == 0

# Sequence indexing access
assert hi[0] == 64
assert hi[1] == 2305843009213693951
assert hi[2] == 314159
assert hi[3] == 0
assert hi[4] == 1000003
assert hi[5] == 'siphash13'
assert hi[6] == 64
assert hi[7] == 128
assert hi[8] == 0
assert hi[-1] == 0

# Slicing
assert hi[0:3] == (64, 2305843009213693951, 314159)

# Unpacking into variables
w, m, inf, nan, im, algo, hb, sb, cut = hi
assert w == 64
assert m == 2305843009213693951
assert algo == 'siphash13'

# Tuple comparison and hashability
assert hi == (64, 2305843009213693951, 314159, 0, 1000003, 'siphash13', 64, 128, 0)
assert hash(hi) == hash(tuple(hi))

# Repr check
assert repr(hi) == (
    "sys.hash_info(width=64, modulus=2305843009213693951, inf=314159, nan=0, "
    "imag=1000003, algorithm='siphash13', hash_bits=64, seed_bits=128, cutoff=0)"
)

# Structseq API restrictions (absence of namedtuple-only methods)
assert hasattr(hi, '_fields') is False
assert hasattr(hi, '_replace') is False
assert hasattr(hi, '_asdict') is False

# Non-existent attribute error
try:
    hi.nonexistent
except AttributeError:
    pass
else:
    raise AssertionError('expected AttributeError for non-existent attribute')

# Read-only attribute assignment error
try:
    hi.width = 32
except AttributeError:
    pass
else:
    raise AssertionError('expected AttributeError for attribute mutation')

# Item mutation error
try:
    hi[0] = 32
except TypeError:
    pass
else:
    raise AssertionError('expected TypeError for item mutation')

# === sys.copyright ===
assert isinstance(sys.copyright, str)
assert len(sys.copyright) > 0

# === sys.pycache_prefix ===
assert sys.pycache_prefix is None

# === sys.builtin_module_names ===
assert isinstance(sys.builtin_module_names, tuple)
assert 'sys' in sys.builtin_module_names
assert list(sys.builtin_module_names) == sorted(sys.builtin_module_names)

# === sys.flags ===
# Same 18 fields as CPython 3.14, in the same order
# CPython keeps three further fields outside the sequence, so 18 is the length
# on both engines
assert len(sys.flags) == 18
assert len(tuple(sys.flags)) == 18
assert sys.flags[0] == sys.flags.debug
assert sys.flags[-1] == sys.flags.int_max_str_digits
assert sys.flags.debug == 0
assert sys.flags.inspect == 0
assert sys.flags.interactive == 0
assert sys.flags.optimize == 0
assert sys.flags.no_user_site == 0
assert sys.flags.no_site == 0
assert sys.flags.ignore_environment == 0
assert sys.flags.verbose == 0
assert sys.flags.bytes_warning == 0
assert sys.flags.quiet == 0
assert sys.flags.isolated == 0
assert sys.flags.dev_mode is False
assert sys.flags.utf8_mode == 0
assert sys.flags.warn_default_encoding == 0
assert sys.flags.safe_path is False
assert sys.flags.int_max_str_digits == 4300
assert repr(sys.flags).startswith('sys.flags(debug=0, ')

# === sys.argv ===
assert isinstance(sys.argv, list)
assert len(sys.argv) >= 1
assert isinstance(sys.argv[0], str)
