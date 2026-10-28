import fnmatch
from pathlib import Path

# === fnmatch() and fnmatchcase() ===
# the sandbox's os.path is posixpath, so both are case-sensitive on both interpreters
assert fnmatch.fnmatch('foo.py', '*.py')
assert not fnmatch.fnmatch('foo.pyc', '*.py')
assert fnmatch.fnmatchcase('foo.py', '*.py')
assert not fnmatch.fnmatchcase('FOO.py', '*.PY')
assert fnmatch.fnmatch('a/b.py', '*.py')
assert fnmatch.fnmatch('.hidden', '*')
assert fnmatch.fnmatch('a\nb', 'a?b')
assert fnmatch.fnmatch('a\n', 'a*')
assert fnmatch.fnmatch('', '*')
assert not fnmatch.fnmatch('', '?')
assert fnmatch.fnmatch('', '')
assert fnmatch.fnmatch('abc', 'a*c*')
assert not fnmatch.fnmatch('ac', 'a*b*c')
assert fnmatch.fnmatch('abxbc', 'a*b*c')
assert fnmatch.fnmatch('xaybz', 'x*y*z*')
assert fnmatch.fnmatch('web-01.example.com', 'web-??.example.com')
assert fnmatch.fnmatch('db-primary', 'db-*')
assert fnmatch.fnmatch('é', '[à-ü]')

# === character classes ===
assert fnmatch.fnmatch('b', '[abc]')
assert not fnmatch.fnmatch('d', '[abc]')
assert fnmatch.fnmatch('d', '[!abc]')
assert fnmatch.fnmatch(']', '[]]')
assert not fnmatch.fnmatch('a]', '[]]')
assert fnmatch.fnmatch('x', '[!]]')
assert fnmatch.fnmatch('-', '[a-]')
assert fnmatch.fnmatch('-', '[-a]')
assert not fnmatch.fnmatch('-', '[!a-]')
assert fnmatch.fnmatch('\\', '[\\]')
assert fnmatch.fnmatch('^', '[^a]')
assert fnmatch.fnmatch('a', '[a^]')
assert fnmatch.fnmatch('[', '[[]')
assert fnmatch.fnmatch('b', '[a-c-e]')
assert fnmatch.fnmatch('-', '[a-c-e]')
assert fnmatch.fnmatch('e', '[a-c-e]')
# reversed ranges are dropped, as CPython's regex translation drops them
assert not fnmatch.fnmatch('b', '[b-a]')
assert not fnmatch.fnmatch('-', '[a--]')
assert fnmatch.fnmatch('-', '[b-a-c]')
assert fnmatch.fnmatch('c', '[b-a-c]')
assert not fnmatch.fnmatch('b', '[b-a-c]')
assert fnmatch.fnmatch('b', '[!b-a]')
assert fnmatch.fnmatch('-', '[!b-a]')
# an unterminated class is a literal `[`
assert fnmatch.fnmatch('[', '[')
assert fnmatch.fnmatch('a[', 'a[')
assert fnmatch.fnmatch('[!]', '[!]')
assert not fnmatch.fnmatch('x', '[!]')
assert fnmatch.fnmatch('[]', '[]')
assert fnmatch.fnmatch('[a', '[a')
# POSIX classes are not special: `[[:upper:]` is a class and the last `]` a literal
assert not fnmatch.fnmatch('A', '[[:upper:]]')
assert fnmatch.fnmatch(':]', '[[:upper:]]')

# === bytes ===
assert fnmatch.fnmatch(b'a.py', b'*.py')
assert fnmatch.fnmatchcase(b'a.py', b'*.[px]?')
assert not fnmatch.fnmatchcase(b'a.pyc', b'*.py')
assert fnmatch.filter([b'a.py', b'b.txt'], b'*.py') == [b'a.py']

# === path objects go through os.fspath in fnmatch() and filter()'s pattern ===
assert fnmatch.fnmatch(Path('/tmp/a.py'), '*.py')
assert fnmatch.fnmatch('a.py', Path('*.py'))
assert fnmatch.filter(['a.py'], Path('*.py')) == ['a.py']

# === filter() ===
assert fnmatch.filter(['a.py', 'b.txt', 'c.py'], '*.py') == ['a.py', 'c.py']
assert fnmatch.filter([], '*') == []
assert fnmatch.filter('abc', '?') == ['a', 'b', 'c']
assert fnmatch.filter(iter(['a', 'b']), '*') == ['a', 'b']
assert fnmatch.filter(('x.log', 'y.log'), '*.log') == ['x.log', 'y.log']
assert fnmatch.filter(names=['a'], pat='*') == ['a']
_hosts = ['web-01', 'web-02', 'db-01']
assert fnmatch.filter(_hosts, 'web-*') == ['web-01', 'web-02']
assert fnmatch.filter(_hosts, '*-01') == ['web-01', 'db-01']

# === translate() ===
for _pat, _regex in [
    ('*.py', '(?s:.*\\.py)\\z'),
    ('?', '(?s:.)\\z'),
    ('[abc]', '(?s:[abc])\\z'),
    ('[!abc]', '(?s:[^abc])\\z'),
    ('[a-z]x', '(?s:[a-z]x)\\z'),
    ('[]]', '(?s:[]])\\z'),
    ('[!]]', '(?s:[^]])\\z'),
    ('[', '(?s:\\[)\\z'),
    ('a*b*c', '(?s:a(?>.*?b).*c)\\z'),
    ('**', '(?s:.*)\\z'),
    ('*a*', '(?s:(?>.*?a).*)\\z'),
    ('x*y*z*', '(?s:x(?>.*?y)(?>.*?z).*)\\z'),
    ('[a-]', '(?s:[a\\-])\\z'),
    ('[-a]', '(?s:[\\-a])\\z'),
    ('[]', '(?s:\\[\\])\\z'),
    ('[[]', '(?s:[\\[])\\z'),
    ('[\\]', '(?s:[\\\\])\\z'),
    ('a.b', '(?s:a\\.b)\\z'),
    ('[a-c-e]', '(?s:[a-c\\-e])\\z'),
    ('[a--]', '(?s:(?!))\\z'),
    ('[--a]', '(?s:[\\--a])\\z'),
    ('[b-a]', '(?s:(?!))\\z'),
    ('[b-a-c]', '(?s:[\\-c])\\z'),
    ('[!b-a]', '(?s:.)\\z'),
    ('[^a]', '(?s:[\\^a])\\z'),
    ('[a^]', '(?s:[a^])\\z'),
    ('a\\b', '(?s:a\\\\b)\\z'),
    ('{a,b}', '(?s:\\{a,b\\})\\z'),
    ('[!]', '(?s:\\[!\\])\\z'),
    ('[a-b-c-d]', '(?s:[a-b\\-c-d])\\z'),
    ('[---]', '(?s:[\\--\\-])\\z'),
    ('[a&b|c~d]', '(?s:[a\\&b\\|c\\~d])\\z'),
    ('\n', '(?s:\\\n)\\z'),
    (' ', '(?s:\\ )\\z'),
    ('é', '(?s:é)\\z'),
    ('', '(?s:)\\z'),
]:
    assert fnmatch.translate(_pat) == _regex
assert fnmatch.translate(pat='*') == '(?s:.*)\\z'

# === errors ===
for _call, _msg in [
    (lambda: fnmatch.fnmatch(1, '*'), 'expected str, bytes or os.PathLike object, not int'),
    (lambda: fnmatch.fnmatch('a', 1), 'expected str, bytes or os.PathLike object, not int'),
    (lambda: fnmatch.fnmatch(b'a', '*'), 'cannot use a string pattern on a bytes-like object'),
    (lambda: fnmatch.fnmatch('a', b'*'), 'cannot use a bytes pattern on a string-like object'),
    (lambda: fnmatch.fnmatchcase(1, '*'), "expected string or bytes-like object, got 'int'"),
    (lambda: fnmatch.fnmatchcase(Path('a'), '*'), "expected string or bytes-like object, got 'PosixPath'"),
    (lambda: fnmatch.fnmatchcase('a', 1), "object of type 'int' has no len()"),
    (lambda: fnmatch.fnmatchcase('a', b'*'), 'cannot use a bytes pattern on a string-like object'),
    (lambda: fnmatch.filter(['a.py', 1], '*.py'), "expected string or bytes-like object, got 'int'"),
    (lambda: fnmatch.filter([b'a.py', 'b.py'], b'*.py'), 'cannot use a bytes pattern on a string-like object'),
    (lambda: fnmatch.filter([], 1), 'expected str, bytes or os.PathLike object, not int'),
    (lambda: fnmatch.filter(5, '*'), "'int' object is not iterable"),
    (lambda: fnmatch.translate(1), "object of type 'int' has no len()"),
    (lambda: fnmatch.translate(b'*'), 'decoding to str: need a bytes-like object, int found'),
    (lambda: fnmatch.fnmatch('a'), "fnmatch() missing 1 required positional argument: 'pat'"),
    (lambda: fnmatch.fnmatch('a', '*', 1), 'fnmatch() takes 2 positional arguments but 3 were given'),
    (lambda: fnmatch.fnmatch('x', foo='*'), "fnmatch() got an unexpected keyword argument 'foo'"),
]:
    try:
        _call()
        assert False, 'expected TypeError'
    except TypeError as e:
        assert str(e) == _msg
