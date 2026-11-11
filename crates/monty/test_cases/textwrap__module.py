# textwrap is vendored from CPython's own source; see crates/monty/stdlib/
import textwrap
from textwrap import TextWrapper, dedent, fill, indent, shorten, wrap

TEXT = 'The quick brown fox jumps over the lazy dog.'

# === wrap and fill ===
assert wrap(TEXT, 20) == ['The quick brown fox', 'jumps over the lazy', 'dog.']
assert wrap(TEXT, 10) == ['The quick', 'brown fox', 'jumps over', 'the lazy', 'dog.']
assert wrap(TEXT) == [TEXT]
assert wrap('') == []
assert wrap('   ') == []
assert fill(TEXT, 20) == 'The quick brown fox\njumps over the lazy\ndog.'
assert (
    fill(TEXT, 20, initial_indent='* ', subsequent_indent='  ')
    == '* The quick brown\n  fox jumps over the\n  lazy dog.'
)
assert textwrap.wrap is wrap

# === whitespace handling ===
assert wrap('a  b\tc\nd', 80) == ['a  b    c d']
assert wrap('a  b\tc\nd', 80, replace_whitespace=False) == ['a  b    c\nd']
assert wrap('one\ttwo', 80, expand_tabs=False) == ['one two']
assert wrap('a\tb', 80, tabsize=4) == ['a   b']
assert wrap('  leading and trailing  ', 80) == ['  leading and trailing']
assert wrap('  leading', 80, drop_whitespace=False) == ['  leading']
assert wrap('hello   world', 8, drop_whitespace=False) == ['hello   ', 'world']

# === long words and hyphens ===
assert wrap('abcdefghijklmnop', 5) == ['abcde', 'fghij', 'klmno', 'p']
assert wrap('abcdefghijklmnop', 5, break_long_words=False) == ['abcdefghijklmnop']
assert wrap('well-known fact', 7) == ['well-', 'known', 'fact']
assert wrap('well-known fact', 7, break_on_hyphens=False) == ['well-kn', 'own', 'fact']

# === sentence endings ===
assert wrap('One. Two.  Three? Four!', 80) == ['One. Two.  Three? Four!']
assert wrap('One. Two.  Three? Four!', 80, fix_sentence_endings=True) == ['One.  Two.  Three?  Four!']

# === max_lines and placeholder ===
assert wrap(TEXT, 20, max_lines=2) == ['The quick brown fox', 'jumps over the [...]']
assert wrap(TEXT, 20, max_lines=2, placeholder='~') == ['The quick brown fox', 'jumps over the lazy~']
assert wrap(TEXT, 20, max_lines=1) == ['The quick [...]']

# === TextWrapper ===
w = TextWrapper(width=15, initial_indent='> ', subsequent_indent='  ')
assert w.wrap(TEXT) == ['> The quick', '  brown fox', '  jumps over', '  the lazy dog.']
assert w.fill('a b') == '> a b'
w.width = 30
assert w.wrap(TEXT) == ['> The quick brown fox jumps', '  over the lazy dog.']
assert TextWrapper().width == 70

# === non-ASCII text ===
assert wrap('héllo wörld café', 11) == ['héllo wörld', 'café']
assert wrap('日本語 のテキスト', 7) == ['日本語', 'のテキスト']

# === dedent ===
assert dedent('    a\n      b\n    c') == 'a\n  b\nc'
assert dedent('  a\n\n  b\n') == 'a\n\nb\n'
assert dedent('a\n  b') == 'a\n  b'
assert dedent('') == ''
assert dedent('\t a\n\t b') == 'a\nb'

# === indent ===
assert indent('a\nb\n', '> ') == '> a\n> b\n'
assert indent('a\n\nb', '> ') == '> a\n\n> b'
assert indent('a\n\nb', '> ', lambda line: True) == '> a\n> \n> b'
assert indent('', '> ') == ''

# === shorten ===
assert shorten('Hello  world!', width=12) == 'Hello world!'
assert shorten('Hello  world!', width=11) == 'Hello [...]'
assert shorten('Hello world', width=10, placeholder='...') == 'Hello...'
assert shorten('   ', width=5) == ''

# === errors ===
try:
    wrap(TEXT, 0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'invalid width 0 (must be > 0)'

try:
    wrap(TEXT, -3)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'invalid width -3 (must be > 0)'

try:
    wrap(TEXT, 4, max_lines=1)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'placeholder too large for max width'
