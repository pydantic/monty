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
