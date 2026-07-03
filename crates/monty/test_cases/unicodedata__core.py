import unicodedata as u

# Precomposed vs decomposed forms are visually identical, so use explicit
# escapes: NFC_E is U+00E9 (é), NFD_E is 'e' + U+0301 (combining acute).
NFC_E = 'é'
NFD_E = 'é'
ACUTE = '́'
FI = 'ﬁ'
UNNAMED = '￿'  # a permanently-unassigned code point (has no name)

# === unidata_version ===
assert u.unidata_version == '16.0.0', 'unicode version matches CPython 3.14'

# === category ===
assert u.category('A') == 'Lu', 'uppercase letter'
assert u.category('a') == 'Ll', 'lowercase letter'
assert u.category('1') == 'Nd', 'decimal number'
assert u.category(' ') == 'Zs', 'space separator'
assert u.category('!') == 'Po', 'other punctuation'
assert u.category(NFC_E) == 'Ll', 'accented lowercase letter'
assert u.category(ACUTE) == 'Mn', 'combining acute is a nonspacing mark'
assert u.category('_') == 'Pc', 'connector punctuation'
assert u.category('+') == 'Sm', 'math symbol'

# === name / lookup ===
assert u.name('A') == 'LATIN CAPITAL LETTER A', 'name of A'
assert u.name(NFC_E) == 'LATIN SMALL LETTER E WITH ACUTE', 'name of e-acute'
assert u.lookup('LATIN SMALL LETTER A') == 'a', 'lookup a'
assert u.lookup('GREEK SMALL LETTER ALPHA') == 'α', 'lookup alpha'
assert u.name(UNNAMED, 'DEFAULT') == 'DEFAULT', 'name default fallback for unnamed char'

# An unknown name raises KeyError whose single arg is the (unquoted) message;
# str() of a KeyError repr-quotes it, matching CPython.
try:
    u.lookup('NOPE NOT A NAME')
    assert False, 'expected lookup of an unknown name to raise'
except KeyError as exc:
    assert exc.args[0] == "undefined character name 'NOPE NOT A NAME'", 'lookup KeyError message'

# === combining ===
assert u.combining('a') == 0, 'ascii letter has no combining class'
assert u.combining(ACUTE) == 230, 'combining acute has class 230'

# === normalize ===
assert u.normalize('NFC', NFD_E) == NFC_E, 'NFC composes e + acute'
assert u.normalize('NFD', NFC_E) == NFD_E, 'NFD decomposes e-acute'
assert u.normalize('NFKC', FI) == 'fi', 'NFKC expands fi ligature'
assert u.normalize('NFKD', FI) == 'fi', 'NFKD expands fi ligature'
assert u.normalize('NFC', '') == '', 'normalize empty string'
assert u.normalize('NFC', 'hello') == 'hello', 'normalize ascii is unchanged'

# === is_normalized ===
assert u.is_normalized('NFC', NFC_E) is True, 'precomposed is NFC'
assert u.is_normalized('NFC', NFD_E) is False, 'decomposed is not NFC'
assert u.is_normalized('NFD', NFD_E) is True, 'decomposed is NFD'
assert u.is_normalized('NFD', NFC_E) is False, 'precomposed is not NFD'

# === error cases ===
# A well-typed but unrecognised form is a *value* error, distinct from the
# type error for a non-str form (regression guard: the two must not be conflated).
try:
    u.normalize('XYZ', 'abc')
    assert False, 'expected invalid form to raise'
except ValueError as exc:
    assert str(exc) == 'invalid normalization form', 'normalize invalid form'
try:
    u.is_normalized('BAD', 'abc')
    assert False, 'expected invalid form to raise'
except ValueError as exc:
    assert str(exc) == 'invalid normalization form', 'is_normalized invalid form'

# All arguments are type-checked before the form's *value* is validated:
# a bad form plus a non-str unistr raises the arg-2 TypeError, not ValueError.
try:
    u.normalize('XYZ', 123)
    assert False, 'expected non-str unistr to win over invalid form'
except TypeError as exc:
    assert str(exc) == 'normalize() argument 2 must be str, not int', 'normalize type check precedes form value check'
try:
    u.is_normalized('XYZ', 123)
    assert False, 'expected non-str unistr to win over invalid form'
except TypeError as exc:
    assert str(exc) == 'is_normalized() argument 2 must be str, not int', (
        'is_normalized type check precedes form value check'
    )

# A non-str form/string is a type error, numbered by argument position.
try:
    u.normalize(123, 'abc')
    assert False, 'expected non-str form to raise'
except TypeError as exc:
    assert str(exc) == 'normalize() argument 1 must be str, not int', 'normalize arg 1 type'
try:
    u.normalize('NFC', 123)
    assert False, 'expected non-str unistr to raise'
except TypeError as exc:
    assert str(exc) == 'normalize() argument 2 must be str, not int', 'normalize arg 2 type'

# Single-character functions distinguish wrong-type from wrong-length.
try:
    u.category(123)
    assert False, 'expected non-str to raise'
except TypeError as exc:
    assert str(exc) == 'category() argument must be a unicode character, not int', 'category type error'
try:
    u.combining('ab')
    assert False, 'expected multi-char string to raise'
except TypeError as exc:
    assert str(exc) == 'combining(): argument must be a unicode character, not a string of length 2', (
        'combining length error'
    )

# name() uses PyArg_UnpackTuple arity wording: a min..max range, so too-few and
# too-many report "at least"/"at most" rather than an exact count.
try:
    u.name()
    assert False, 'expected name() with no args to raise'
except TypeError as exc:
    assert str(exc) == 'name expected at least 1 argument, got 0', 'name too few args'
try:
    u.name('a', 'b', 'c')
    assert False, 'expected name() with 3 args to raise'
except TypeError as exc:
    assert str(exc) == 'name expected at most 2 arguments, got 3', 'name too many args'
