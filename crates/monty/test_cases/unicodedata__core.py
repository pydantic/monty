import unicodedata as u

# Precomposed vs decomposed forms are visually identical, so use explicit
# escapes: NFC_E is U+00E9 (é), NFD_E is 'e' + U+0301 (combining acute).
NFC_E = 'é'
NFD_E = 'é'
ACUTE = '́'
FI = 'ﬁ'
UNNAMED = '￿'  # a permanently-unassigned code point (has no name)

# === unidata_version ===
assert u.unidata_version == '16.0.0'

# === category ===
assert u.category('A') == 'Lu'
assert u.category('a') == 'Ll'
assert u.category('1') == 'Nd'
assert u.category(' ') == 'Zs'
assert u.category('!') == 'Po'
assert u.category(NFC_E) == 'Ll'
assert u.category(ACUTE) == 'Mn'
assert u.category('_') == 'Pc'
assert u.category('+') == 'Sm'

# === name / lookup ===
assert u.name('A') == 'LATIN CAPITAL LETTER A'
assert u.name(NFC_E) == 'LATIN SMALL LETTER E WITH ACUTE'
assert u.lookup('LATIN SMALL LETTER A') == 'a'
assert u.lookup('GREEK SMALL LETTER ALPHA') == 'α'
assert u.name(UNNAMED, 'DEFAULT') == 'DEFAULT'

# An unknown name raises KeyError whose single arg is the (unquoted) message;
# str() of a KeyError repr-quotes it, matching CPython.
try:
    u.lookup('NOPE NOT A NAME')
    assert False, 'expected lookup of an unknown name to raise'
except KeyError as exc:
    assert exc.args[0] == "undefined character name 'NOPE NOT A NAME'"

# === combining ===
assert u.combining('a') == 0
assert u.combining(ACUTE) == 230

# === normalize ===
assert u.normalize('NFC', NFD_E) == NFC_E
assert u.normalize('NFD', NFC_E) == NFD_E
assert u.normalize('NFKC', FI) == 'fi'
assert u.normalize('NFKD', FI) == 'fi'
assert u.normalize('NFC', '') == ''
assert u.normalize('NFC', 'hello') == 'hello'

# === is_normalized ===
assert u.is_normalized('NFC', NFC_E) is True
assert u.is_normalized('NFC', NFD_E) is False
assert u.is_normalized('NFD', NFD_E) is True
assert u.is_normalized('NFD', NFC_E) is False

# === error cases ===
# A well-typed but unrecognised form is a *value* error, distinct from the
# type error for a non-str form (regression guard: the two must not be conflated).
try:
    u.normalize('XYZ', 'abc')
    assert False, 'expected invalid form to raise'
except ValueError as exc:
    assert str(exc) == 'invalid normalization form'
try:
    u.is_normalized('BAD', 'abc')
    assert False, 'expected invalid form to raise'
except ValueError as exc:
    assert str(exc) == 'invalid normalization form'

# All arguments are type-checked before the form's *value* is validated:
# a bad form plus a non-str unistr raises the arg-2 TypeError, not ValueError.
try:
    u.normalize('XYZ', 123)
    assert False, 'expected non-str unistr to win over invalid form'
except TypeError as exc:
    assert str(exc) == 'normalize() argument 2 must be str, not int'
try:
    u.is_normalized('XYZ', 123)
    assert False, 'expected non-str unistr to win over invalid form'
except TypeError as exc:
    assert str(exc) == 'is_normalized() argument 2 must be str, not int'

# A non-str form/string is a type error, numbered by argument position.
try:
    u.normalize(123, 'abc')
    assert False, 'expected non-str form to raise'
except TypeError as exc:
    assert str(exc) == 'normalize() argument 1 must be str, not int'
try:
    u.normalize('NFC', 123)
    assert False, 'expected non-str unistr to raise'
except TypeError as exc:
    assert str(exc) == 'normalize() argument 2 must be str, not int'

# Single-character functions distinguish wrong-type from wrong-length.
try:
    u.category(123)
    assert False, 'expected non-str to raise'
except TypeError as exc:
    assert str(exc) == 'category() argument must be a unicode character, not int'
try:
    u.combining('ab')
    assert False, 'expected multi-char string to raise'
except TypeError as exc:
    assert str(exc) == 'combining(): argument must be a unicode character, not a string of length 2'

# name() uses PyArg_UnpackTuple arity wording: a min..max range, so too-few and
# too-many report "at least"/"at most" rather than an exact count.
try:
    u.name()
    assert False, 'expected name() with no args to raise'
except TypeError as exc:
    assert str(exc) == 'name expected at least 1 argument, got 0'
try:
    u.name('a', 'b', 'c')
    assert False, 'expected name() with 3 args to raise'
except TypeError as exc:
    assert str(exc) == 'name expected at most 2 arguments, got 3'
