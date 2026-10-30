# Tests for regex pattern syntax: what CPython accepts, what it rejects and with which message

import re


def pattern_error(pattern, flags=0):
    try:
        re.compile(pattern, flags)
    except re.PatternError as e:
        return str(e)
    assert False, f'expected PatternError for {pattern!r}'


# === Subroutine calls and recursion are not CPython syntax ===
assert pattern_error('(a)\\g<1>') == 'bad escape \\g at position 3'
assert pattern_error("(a)\\g'1'") == 'bad escape \\g at position 3'
assert pattern_error('(?<a>x\\g<a>?\\g<a>?)') == 'unknown extension ?<a at position 1'
assert pattern_error('(a\\g<1>?\\g<1>?\\g<1>?)') == 'bad escape \\g at position 2'
assert pattern_error('(?P<n>a)(?P>n)') == 'unknown extension ?P> at position 9'
assert pattern_error('(?R)') == 'unknown extension ?R at position 1'
assert pattern_error('(?&n)') == 'unknown extension ?& at position 1'
assert pattern_error("(?'a'x)") == "unknown extension ?' at position 1"
assert pattern_error('(?~abc)') == 'unknown extension ?~ at position 1'
assert pattern_error('(*FAIL)') == 'nothing to repeat at position 1'

# === Escapes CPython does not define ===
assert pattern_error('\\R') == 'bad escape \\R at position 0'
assert pattern_error('\\O') == 'bad escape \\O at position 0'
assert pattern_error('\\K') == 'bad escape \\K at position 0'
assert pattern_error('a\\G') == 'bad escape \\G at position 1'
assert pattern_error('\\h') == 'bad escape \\h at position 0'
assert pattern_error('\\p{L}') == 'bad escape \\p at position 0'
assert pattern_error('[\\p{L}]') == 'bad escape \\p at position 1'
assert pattern_error('\\e') == 'bad escape \\e at position 0'
assert pattern_error('\\k<n>') == 'bad escape \\k at position 0'
assert pattern_error('a\\') == 'bad escape (end of pattern) at position 1'

# === Escaped punctuation is a literal ===
assert re.findall('\\<a\\>', '<a> a') == ['<a>']
assert re.findall('\\-\\#\\ ', '-# ') == ['-# ']
assert re.findall('\\é', 'é') == ['é']

# === \Z is the very end, like \z ===
assert re.search('a\\Z', 'a\n') is None
assert re.search('a\\z', 'a') is not None

# === Numeric escapes ===
assert re.findall('\\x41\\u0042\\U00000043', 'ABC') == ['ABC']
assert pattern_error('\\x4') == 'incomplete escape \\x4 at position 0'
assert pattern_error('\\x{41}') == 'incomplete escape \\x at position 0'
assert pattern_error('\\U00110000') == 'bad escape \\U00110000 at position 0'
assert re.findall('\\141\\0', 'a\x00') == ['a\x00']
assert re.findall('[\\1]', '\x01') == ['\x01']
assert pattern_error('\\400') == 'octal escape value \\400 outside of range 0-0o377 at position 0'
assert pattern_error('[\\8]') == 'bad escape \\8 at position 1'

# === Named characters ===
assert re.findall('\\N{LATIN SMALL LETTER A}', 'abc') == ['a']
assert re.findall('[\\N{EM DASH}b]', 'a—b') == ['—', 'b']
assert re.sub('\\N{EM DASH}', '-', 'a—b') == 'a-b'
assert pattern_error('\\N') == 'missing { at position 2'
assert pattern_error('\\N{') == 'missing character name at position 3'
assert pattern_error('\\N{}') == 'missing character name at position 3'
assert pattern_error('\\N{NOPE}') == "undefined character name 'NOPE' at position 0"

# === Backreferences ===
assert re.findall('(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\\10', 'abcdefghijj') == [tuple('abcdefghij')]
assert pattern_error('(a)\\2') == 'invalid group reference 2 at position 4'
assert pattern_error('(a\\1)') == 'cannot refer to an open group at position 2'
assert pattern_error('(?P=n)') == "unknown group name 'n' at position 4"

# === Group names ===
assert pattern_error('(?P<a>x)(?P<a>y)') == "redefinition of group name 'a' as group 2; was group 1 at position 12"
assert pattern_error('(?P<n>a)|(?P<n>b)') == "redefinition of group name 'n' as group 2; was group 1 at position 13"
assert pattern_error('(?P<a b>x)') == "bad character in group name 'a b' at position 4"
assert pattern_error('(?P<a-b>x)') == "bad character in group name 'a-b' at position 4"
assert pattern_error('(?P<1a>x)') == "bad character in group name '1a' at position 4"
assert pattern_error('(?P<>x)') == 'missing group name at position 4'
assert pattern_error('(?P<a') == 'missing >, unterminated name at position 4'
assert re.match('(?P<é>a)', 'a').groupdict() == {'é': 'a'}
assert re.sub('(?P<é>a)', '\\g<é>\\g<é>', 'ba') == 'baa'

# === Repeats ===
assert pattern_error('a{3,2}') == 'min repeat greater than max repeat at position 2'
assert pattern_error('a**') == 'multiple repeat at position 2'
assert pattern_error('a+?+') == 'multiple repeat at position 3'
assert pattern_error('a{2}{3}') == 'multiple repeat at position 4'
assert pattern_error('{2}') == 'nothing to repeat at position 0'
assert pattern_error('^*') == 'nothing to repeat at position 1'
assert re.findall('x{', 'x{') == ['x{']
assert re.findall('a{,2}b', 'aaab') == ['aab']
try:
    re.compile('a{4294967295}')
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'the repetition number is too large'

# === Character classes ===
assert re.findall('[a&&b]', 'a&b') == ['a', '&', 'b']
assert re.findall('[a||b]', 'a|') == ['a', '|']
assert re.findall('[[a]]', 'a]') == ['a]']
assert re.findall('[]a]', ']a') == [']', 'a']
assert re.findall('[\\b]', '\x08b') == ['\x08']
assert pattern_error('[a--b]') == 'bad character range a-- at position 1'
assert pattern_error('[\\d-z]') == 'bad character range \\d-z at position 1'
assert pattern_error('[z-a]') == 'bad character range z-a at position 1'
assert pattern_error('[a') == 'unterminated character set at position 0'

# === Conditionals ===
assert re.findall('(a)?(?(1)b|c)', 'ab c') == ['a', '']
assert pattern_error('(?(1)a|b)') == 'invalid group reference 1 at position 3'
assert pattern_error('(?(n)a|b)') == "unknown group name 'n' at position 3"
assert pattern_error('(?(0)a)') == 'bad group number at position 3'
assert pattern_error('(a)(?(1)a|b|c)') == 'conditional backref with more than two branches at position 11'

# === Lookbehind must be fixed width ===
assert re.findall('(?<=ab|cd)e', 'abe cde') == ['e', 'e']
assert pattern_error('(?<=a+)b') == 'look-behind requires fixed-width pattern'
assert pattern_error('(?<=a|bc)d') == 'look-behind requires fixed-width pattern'
assert pattern_error('(?<=(a)\\1)b') == 'cannot refer to group defined in the same lookbehind subpattern at position 9'

# === Inline flags ===
assert re.findall('(?i)a', 'aA') == ['a', 'A']
assert re.findall('(?i:a)a', 'Aa AA') == ['Aa']
assert re.findall('(?x) a b # comment\n c', 'abc') == ['abc']
assert re.findall('(?x)[ a]', ' a') == [' ', 'a']
assert re.findall('(?x)a\\ b', 'a b') == ['a b']
assert pattern_error('a(?i)b') == 'global flags not at the start of the expression at position 1'
assert pattern_error('(?L)a') == "bad inline flags: cannot use 'L' flag with a str pattern at position 3"
assert pattern_error('(?au)a') == "bad inline flags: flags 'a', 'u' and 'L' are incompatible at position 4"
assert pattern_error('(?i-i:a)') == 'bad inline flags: flag turned on and off at position 5'
assert pattern_error('(?q)') == 'unknown extension ?q at position 1'

# === Parentheses ===
assert pattern_error('a)') == 'unbalanced parenthesis at position 1'
assert pattern_error('(a') == 'missing ), unterminated subpattern at position 0'
assert pattern_error('(?#c') == 'missing ), unterminated comment at position 0'
assert pattern_error('a\nb(') == 'missing ), unterminated subpattern at position 3 (line 2, column 2)'
