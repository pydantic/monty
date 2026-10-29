import urllib
import urllib.parse
from urllib.parse import (
    parse_qs,
    parse_qsl,
    quote,
    quote_from_bytes,
    quote_plus,
    unquote,
    unquote_plus,
    unquote_to_bytes,
    urldefrag,
    urlencode,
    urljoin,
    urlparse,
    urlsplit,
    urlunparse,
    urlunsplit,
)

assert urllib.parse.urlparse is urlparse

# === urlparse / urlsplit ===
_url = 'http://user:pw@Example.COM:8080/a/b;p?x=1&y=2#frag'
_r = urlparse(_url)
assert _r == ('http', 'user:pw@Example.COM:8080', '/a/b', 'p', 'x=1&y=2', 'frag')
assert repr(_r) == (
    "ParseResult(scheme='http', netloc='user:pw@Example.COM:8080', path='/a/b', params='p', query='x=1&y=2', "
    "fragment='frag')"
)
assert (_r.scheme, _r.netloc, _r.path, _r.params, _r.query, _r.fragment) == tuple(_r)
assert (_r.hostname, _r.port, _r.username, _r.password) == ('example.com', 8080, 'user', 'pw')
assert _r.geturl() == _url
assert _r._fields == ('scheme', 'netloc', 'path', 'params', 'query', 'fragment')
assert _r._replace(scheme='https', fragment='').geturl() == 'https://user:pw@Example.COM:8080/a/b;p?x=1&y=2'
assert _r._asdict()['netloc'] == 'user:pw@Example.COM:8080'
assert len(_r) == 6
assert _r[0] == 'http'
assert _r[1:3] == ('user:pw@Example.COM:8080', '/a/b')

_s = urlsplit(_url)
assert repr(_s) == (
    "SplitResult(scheme='http', netloc='user:pw@Example.COM:8080', path='/a/b;p', query='x=1&y=2', fragment='frag')"
)
assert (_s.hostname, _s.port, _s.username, _s.password) == ('example.com', 8080, 'user', 'pw')
assert _s.geturl() == _url
_scheme, _netloc, *_rest = _s
assert (_scheme, _netloc, _rest) == ('http', 'user:pw@Example.COM:8080', ['/a/b;p', 'x=1&y=2', 'frag'])

for _u, _expected in [
    ('https://[::1]:443/x', ('https', '[::1]:443', '/x', '', '', '')),
    ('http://[fe80::1%25eth0]/', ('http', '[fe80::1%25eth0]', '/', '', '', '')),
    ('mailto:a@b.com', ('mailto', '', 'a@b.com', '', '', '')),
    ('//host/path', ('', 'host', '/path', '', '', '')),
    ('/rel/path?q', ('', '', '/rel/path', '', 'q', '')),
    ('HTTP://x', ('http', 'x', '', '', '', '')),
    ('x://y;z', ('x', 'y;z', '', '', '', '')),
    ('http://x/a;b;c', ('http', 'x', '/a', 'b;c', '', '')),
    ('http://x?q#f', ('http', 'x', '', '', 'q', 'f')),
    ('file:///etc/hosts', ('file', '', '/etc/hosts', '', '', '')),
    ('http://x/#', ('http', 'x', '/', '', '', '')),
    ('  http://x/ ', ('http', 'x', '/ ', '', '', '')),
    ('http://x\n/y', ('http', 'x', '/y', '', '', '')),
    ('a:b', ('a', '', 'b', '', '', '')),
    ('1http://x', ('', '', '1http://x', '', '', '')),
    ('tel:+1;phone-context=x', ('tel', '', '+1', 'phone-context=x', '', '')),
    ('git+ssh://g/r.git', ('git+ssh', 'g', '/r.git', '', '', '')),
    ('', ('', '', '', '', '', '')),
    ('?q', ('', '', '', '', 'q', '')),
]:
    assert urlparse(_u) == _expected
    assert (
        urlsplit(_u)
        == (_expected[0], _expected[1], _expected[2] + (';' + _expected[3] if _expected[3] else '')) + _expected[4:]
    )

assert urlparse('http://x', 'https').scheme == 'http'
assert urlparse('//x/y', 'https') == ('https', 'x', '/y', '', '', '')
assert urlparse('x/y', scheme='ftp') == ('ftp', '', 'x/y', '', '', '')
assert urlparse('http://x/y#z', allow_fragments=False) == ('http', 'x', '/y#z', '', '', '')
assert urlparse('http://x/y#z', '', 0).path == '/y#z'

# netloc properties
assert urlparse('http://x').port is None
assert urlparse('http://x:').port is None
assert urlparse('http://@host').username == ''
assert urlparse('http://host').username is None
assert urlparse('http://u@host').password is None
assert urlparse('https://[::1]:443/x').hostname == '::1'
assert urlparse('http://[fe80::1%25eth0]/').hostname == 'fe80::1%25eth0'
assert urlparse('http://[fe80::1%TeSt]/').hostname == 'fe80::1%TeSt'
assert urlparse('/rel').hostname is None

for _u, _msg in [
    ('http://[::1', 'Invalid IPv6 URL'),
    ('http://::1]', 'Invalid IPv6 URL'),
    ('http://a[::1]', 'Invalid IPv6 URL'),
    ('http://[::1]x', 'Invalid IPv6 URL'),
    ('http://[1.2.3.4]', 'An IPv4 address cannot be in brackets'),
    ('http://[vz.x]', 'IPvFuture address is invalid'),
    ('http://[zz]', "'zz' does not appear to be an IPv4 or IPv6 address"),
    ('http://[fe80::1%]', "'fe80::1%' does not appear to be an IPv4 or IPv6 address"),
    ('http://\u2100host/', "netloc '\u2100host' contains invalid characters under NFKC normalization"),
]:
    try:
        urlparse(_u)
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == _msg
assert urlparse('http://[v1.x]').hostname == 'v1.x'
assert urlparse('http://ex\uff01ample/').hostname == 'ex\uff01ample'

for _call, _msg in [
    (lambda: urlparse('http://x:abc/').port, "Port could not be cast to integer value as 'abc'"),
    (lambda: urlparse('http://x:99999/').port, 'Port out of range 0-65535'),
    (lambda: urlparse('http://x:-1/').port, "Port could not be cast to integer value as '-1'"),
]:
    try:
        _call()
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == _msg

for _call, _exc, _msg in [
    (lambda: urlparse(5), AttributeError, "'int' object has no attribute 'decode'"),
    (lambda: urlparse('x', 5), TypeError, 'Cannot mix str and non-str arguments'),
    (lambda: urlparse('x', 0), AttributeError, "'int' object has no attribute 'strip'"),
    (lambda: urlparse(), TypeError, "urlparse() missing 1 required positional argument: 'url'"),
    (lambda: urlsplit(), TypeError, "urlsplit() missing 1 required positional argument: 'url'"),
    (
        lambda: urlparse('a', 'b', True, 1),
        TypeError,
        'urlparse() takes from 1 to 3 positional arguments but 4 were given',
    ),
    (lambda: _r.nope, AttributeError, "'ParseResult' object has no attribute 'nope'"),
    (lambda: _r.geturl(1), TypeError, 'ParseResult.geturl() takes 1 positional argument but 2 were given'),
    (lambda: urldefrag('http://x/y#f').hostname, AttributeError, "'DefragResult' object has no attribute 'hostname'"),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === urlunsplit / urlunparse ===
assert urlunsplit(('http', 'x', 'p', 'q', 'f')) == 'http://x/p?q#f'
assert urlunsplit(('http', '', 'p', '', '')) == 'http:p'
assert urlunsplit(('http', '', '', '', '')) == 'http://'
assert urlunsplit(('http', '', '/p', '', '')) == 'http:///p'
assert urlunsplit(('mailto', '', 'a@b', '', '')) == 'mailto:a@b'
assert urlunsplit(('', '', '//p', '', '')) == '////p'
assert urlunsplit(('', 'h', 'p', '', '')) == '//h/p'
assert urlunsplit(['http', 'x', '', '', '']) == 'http://x'
assert urlunparse(('http', 'x', 'p', 'pa', 'q', 'f')) == 'http://x/p;pa?q#f'
assert urlunparse(('http', 'x', 'p', '', 'q', 'f')) == 'http://x/p?q#f'
assert urlunparse(urlparse('http://x/y;p?q#f')) == 'http://x/y;p?q#f'
assert urlunsplit(urlsplit('HTTP://X/y')) == 'http://X/y'

for _call, _exc, _msg in [
    (lambda: urlunsplit(('http', 'x', 'p', 'q')), ValueError, 'not enough values to unpack (expected 6, got 5)'),
    (
        lambda: urlunsplit(('http', 'x', 'p', 'q', 'f', 'g')),
        ValueError,
        'too many values to unpack (expected 6, got 7)',
    ),
    (lambda: urlunparse(('http', 'x', 'p', 'pa', 'q')), ValueError, 'not enough values to unpack (expected 7, got 6)'),
    (lambda: urlunparse(('http', 'x', 5, '', '', '')), TypeError, 'Cannot mix str and non-str arguments'),
    (lambda: urlunsplit(5), TypeError, 'urllib.parse._coerce_args() argument after * must be an iterable, not int'),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === urljoin ===
_base = 'http://a/b/c/d;p?q'
for _ref, _expected in [
    ('g', 'http://a/b/c/g'),
    ('./g', 'http://a/b/c/g'),
    ('g/', 'http://a/b/c/g/'),
    ('/g', 'http://a/g'),
    ('//g', 'http://g'),
    ('?y', 'http://a/b/c/d;p?y'),
    ('g?y', 'http://a/b/c/g?y'),
    ('#s', 'http://a/b/c/d;p?q#s'),
    ('g#s', 'http://a/b/c/g#s'),
    ('g?y#s', 'http://a/b/c/g?y#s'),
    (';x', 'http://a/b/c/;x'),
    ('g;x', 'http://a/b/c/g;x'),
    ('', 'http://a/b/c/d;p?q'),
    ('.', 'http://a/b/c/'),
    ('./', 'http://a/b/c/'),
    ('..', 'http://a/b/'),
    ('../', 'http://a/b/'),
    ('../g', 'http://a/b/g'),
    ('../..', 'http://a/'),
    ('../../', 'http://a/'),
    ('../../g', 'http://a/g'),
    ('../../../g', 'http://a/g'),
    ('/./g', 'http://a/g'),
    ('/../g', 'http://a/g'),
    ('g.', 'http://a/b/c/g.'),
    ('.g', 'http://a/b/c/.g'),
    ('..g', 'http://a/b/c/..g'),
    ('./../g', 'http://a/b/g'),
    ('./g/.', 'http://a/b/c/g/'),
    ('g/./h', 'http://a/b/c/g/h'),
    ('g/../h', 'http://a/b/c/h'),
    ('g;x=1/./y', 'http://a/b/c/g;x=1/y'),
    ('g;x=1/../y', 'http://a/b/c/y'),
    ('g?y/./x', 'http://a/b/c/g?y/./x'),
    ('g#s/../x', 'http://a/b/c/g#s/../x'),
    ('http:g', 'http://a/b/c/g'),
    ('https://z/', 'https://z/'),
]:
    assert urljoin(_base, _ref) == _expected
assert urljoin('http://a/b/c/d;p?q#f', '') == 'http://a/b/c/d;p?q#f'
assert urljoin('http://a/b/c/d;p?q#f', '?') == 'http://a/b/c/d;p?'
assert urljoin('http://a', 'x') == 'http://a/x'
assert urljoin('', 'http://b') == 'http://b'
assert urljoin('a/b', 'c') == 'a/c'
assert urljoin('mailto:x', 'y') == 'y'
assert urljoin('http://a/b/', 'c//d') == 'http://a/b/c/d'
assert urljoin('http://a/b/c', '../../../../d') == 'http://a/d'
assert urljoin('file:///a/b', 'c') == 'file:///a/c'
assert urljoin('http://a/b/c/', 'http:') == 'http://a/b/c/'
assert urljoin('http://a/#f', 'x', allow_fragments=False) == 'http://a/x'
assert urljoin('', 5) == 5
assert urljoin(5, '') == 5

for _call, _exc, _msg in [
    (lambda: urljoin('http://a', 5), TypeError, 'Cannot mix str and non-str arguments'),
    (lambda: urljoin(5, 'x'), TypeError, 'Cannot mix str and non-str arguments'),
]:
    try:
        _call()
        assert False, 'expected TypeError'
    except _exc as e:
        assert str(e) == _msg

# a lone segment and a scheme outside `uses_relative`
assert urljoin('a', 'b') == 'b'
assert urljoin('a/', 'b') == 'a/b'
assert urljoin('itms-services://host/a', 'b') == 'b'
assert urljoin('itms-services://host/a', '//other/b') == '//other/b'

# === urldefrag ===
assert urldefrag('http://x/y#f') == ('http://x/y', 'f')
assert repr(urldefrag('http://x/y#f')) == "DefragResult(url='http://x/y', fragment='f')"
assert urldefrag('http://x/y#f').geturl() == 'http://x/y#f'
assert urldefrag('http://x/y').geturl() == 'http://x/y'
assert urldefrag('HTTP://x/y?q#') == ('http://x/y?q', '')
assert urldefrag('http://x/y#f').url == 'http://x/y'

# === quote ===
assert quote('a b/c?d=e&f=g') == 'a%20b/c%3Fd%3De%26f%3Dg'
assert quote_plus('a b/c?d=e&f=g') == 'a+b%2Fc%3Fd%3De%26f%3Dg'
assert quote('über') == '%C3%BCber'
assert quote('a+b') == 'a%2Bb'
assert quote_plus('a+b') == 'a%2Bb'
assert quote_plus('a+b', safe='+') == 'a+b'
assert quote('/~x/') == '/~x/'
assert quote('/~x/', safe='') == '%2F~x%2F'
assert quote('') == ''
assert quote('a b', safe='/+') == 'a%20b'
assert quote('a\u0000b') == 'a%00b'
assert quote(b'a b/\xff') == 'a%20b/%FF'
assert quote_plus(b'a b+c') == 'a+b%2Bc'
assert quote('a b', safe=b'/ ') == 'a b'
assert quote('a b', safe='ä') == 'a%20b'
assert quote('ä', encoding='ascii', errors='ignore') == ''
assert quote_from_bytes(b'a b') == 'a%20b'
assert quote_from_bytes(b'') == ''
assert quote_from_bytes(b'a/b', safe='') == 'a%2Fb'

for _call, _exc, _msg in [
    (lambda: quote(b'a b', encoding='utf-8'), TypeError, "quote() doesn't support 'encoding' for bytes"),
    (lambda: quote(b'a b', errors='strict'), TypeError, "quote() doesn't support 'errors' for bytes"),
    (
        lambda: quote('ä', encoding='ascii'),
        UnicodeEncodeError,
        "'ascii' codec can't encode character '\\xe4' in position 0: ordinal not in range(128)",
    ),
    (lambda: quote('ä', encoding='bogus'), LookupError, 'unknown encoding: bogus'),
    (lambda: quote(5), TypeError, 'quote_from_bytes() expected bytes'),
    (lambda: quote(None), TypeError, 'quote_from_bytes() expected bytes'),
    (lambda: quote('a b', safe=None), TypeError, "'NoneType' object is not iterable"),
    (lambda: quote_from_bytes('a b'), TypeError, 'quote_from_bytes() expected bytes'),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === unquote ===
assert unquote('a%20b') == 'a b'
assert unquote('%C3%BCber') == 'über'
assert unquote('a+b%2Bc') == 'a+b+c'
assert unquote_plus('a+b%2Bc') == 'a b+c'
assert unquote('%') == '%'
assert unquote('%zz') == '%zz'
assert unquote('%C3') == '\ufffd'
assert unquote('%E2%82%AC') == '€'
assert unquote('ä%20b') == 'ä b'
assert unquote('%41%42') == 'AB'
assert unquote('a%%20') == 'a% '
assert unquote('%ff') == '\ufffd'
assert unquote(b'%C3%BC+x') == 'ü+x'
assert unquote_to_bytes('a%20b') == b'a b'
assert unquote_to_bytes('%C3') == b'\xc3'
assert unquote_to_bytes(b'%C3%BC') == b'\xc3\xbc'
assert unquote_to_bytes('ä%20') == b'\xc3\xa4 '
_plain = 'abc'
assert unquote(_plain) is _plain

for _call, _exc, _msg in [
    (
        lambda: unquote('%ff', errors='strict'),
        UnicodeDecodeError,
        "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
    ),
    (lambda: unquote('%ff', 'bogus'), LookupError, 'unknown encoding: bogus'),
    (lambda: unquote(5), TypeError, "argument of type 'int' is not a container or iterable"),
    (lambda: unquote(None), TypeError, "argument of type 'NoneType' is not a container or iterable"),
    (lambda: unquote_plus(b'a+b'), TypeError, "a bytes-like object is required, not 'str'"),
    (lambda: unquote_to_bytes(5), AttributeError, "'int' object has no attribute 'split'"),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === parse_qs / parse_qsl ===
assert parse_qs('a=1&b=2&a=3') == {'a': ['1', '3'], 'b': ['2']}
assert parse_qsl('a=1&b=2&a=3') == [('a', '1'), ('b', '2'), ('a', '3')]
assert parse_qsl('a=1&&b=2') == [('a', '1'), ('b', '2')]
assert parse_qsl('a&b=2') == [('b', '2')]
assert parse_qsl('a=&b=2') == [('b', '2')]
assert parse_qsl('a=&b=2', keep_blank_values=True) == [('a', ''), ('b', '2')]
assert parse_qsl('a&b=2', True) == [('a', ''), ('b', '2')]
assert parse_qsl('a=1;b=2') == [('a', '1;b=2')]
assert parse_qsl('a=1;b=2', separator=';') == [('a', '1'), ('b', '2')]
assert parse_qsl('x=%C3%BC+y') == [('x', 'ü y')]
assert parse_qsl('') == []
assert parse_qs('') == {}
assert parse_qsl('a=b=c') == [('a', 'b=c')]
assert parse_qsl(None) == []
assert parse_qsl(0) == []
assert parse_qsl(b'a=%C3%BC+x&b=2') == [(b'a', b'\xc3\xbc x'), (b'b', b'2')]
assert parse_qs(b'a=1&a=2') == {b'a': [b'1', b'2']}
assert parse_qsl('a=1&b=2', max_num_fields=2) == [('a', '1'), ('b', '2')]
assert parse_qsl('a=1&b', strict_parsing=False) == [('a', '1')]

for _call, _exc, _msg in [
    (lambda: parse_qsl('a&b=2', strict_parsing=True), ValueError, "bad query field: 'a'"),
    (lambda: parse_qsl('a=1&&b', strict_parsing=True), ValueError, "bad query field: ''"),
    (lambda: parse_qsl('a=1&b=2', max_num_fields=1), ValueError, 'Max number of fields exceeded'),
    (lambda: parse_qsl('a=1', separator=''), ValueError, 'Separator must be of type string or bytes.'),
    (lambda: parse_qsl('a=1', separator=5), ValueError, 'Separator must be of type string or bytes.'),
    (lambda: parse_qsl(5), TypeError, "memoryview: a bytes-like object is required, not 'int'"),
    (
        lambda: parse_qsl('x=%ff', errors='strict'),
        UnicodeDecodeError,
        "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
    ),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === urlencode ===
assert urlencode({'a': 1, 'b': 'x y', 'c': [1, 2]}) == 'a=1&b=x+y&c=%5B1%2C+2%5D'
assert urlencode({'a': 1, 'b': 'x y', 'c': [1, 2]}, doseq=True) == 'a=1&b=x+y&c=1&c=2'
assert urlencode([('a', 'ü'), ('b', b'\xff'), (b'k', 'v')]) == 'a=%C3%BC&b=%FF&k=v'
assert urlencode({'a': 'x/y'}, safe='/') == 'a=x/y'
assert urlencode({'a': 'x y'}, quote_via=quote) == 'a=x%20y'
assert urlencode({'a': 'x y'}, quote_via=lambda s, safe, enc, err: s.upper()) == 'A=X Y'
assert urlencode({'a': ('x', 'y')}, True) == 'a=x&a=y'
assert urlencode({'a': 'xy'}, True) == 'a=xy'
assert urlencode({'a': {'k': 1}}, True) == 'a=k'
assert urlencode({'a': None}, True) == 'a=None'
assert urlencode([]) == ''
assert urlencode({}) == ''
assert urlencode(()) == ''
assert urlencode({1: 2, None: True}) == '1=2&None=True'
assert urlencode({'a': [1, 2]}, doseq=1) == 'a=1&a=2'

for _call, _exc, _msg in [
    (lambda: urlencode('abc'), TypeError, 'not a valid non-string sequence or mapping object'),
    (lambda: urlencode(5), TypeError, 'not a valid non-string sequence or mapping object'),
    (lambda: urlencode([1, 2]), TypeError, 'not a valid non-string sequence or mapping object'),
    (lambda: urlencode([['a', 1]]), TypeError, 'not a valid non-string sequence or mapping object'),
    (lambda: urlencode([('a',)]), ValueError, 'not enough values to unpack (expected 2, got 1)'),
    (lambda: urlencode([('a', 1, 2)]), ValueError, 'too many values to unpack (expected 2, got 3)'),
    (
        lambda: urlencode({'a': 'x y'}, quote_via=lambda *a: 5),
        TypeError,
        "unsupported operand type(s) for +: 'int' and 'str'",
    ),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# === results of another shape are not URL results ===
from collections import namedtuple

_Other = namedtuple('SplitResult', ['a'])
try:
    _Other(1).geturl()
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'SplitResult' object has no attribute 'geturl'"
assert urlsplit('x')._replace(netloc=None).path == 'x'
try:
    urlsplit('http://h/').geturl(x=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "SplitResult.geturl() got an unexpected keyword argument 'x'"
try:
    urlsplit("http://h:a'b/").port
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == '''Port could not be cast to integer value as "a'b"'''

# === codec arguments follow str.encode / bytes.decode ===
assert quote('/a b', safe=[47]) == '/a%20b'
assert quote('/a b', safe=(47, 300, 32)) == '/a b'
for _call, _exc, _msg in [
    (lambda: unquote(b'%ff', encoding=None), TypeError, "decode() argument 'encoding' must be str, not None"),
    (lambda: unquote(b'%ff', errors=None), TypeError, "decode() argument 'errors' must be str, not None"),
    (lambda: unquote('%ff', 5), TypeError, "decode() argument 'encoding' must be str, not int"),
    (lambda: quote(1, encoding='x'), TypeError, "quote() doesn't support 'encoding' for bytes"),
    (lambda: quote('a', safe=['/']), TypeError, "'<' not supported between instances of 'str' and 'int'"),
    (lambda: quote('a', safe=[1.5]), TypeError, "'float' object cannot be interpreted as an integer"),
    (lambda: quote('a', safe=[-1]), ValueError, 'bytes must be in range(0, 256)'),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg
assert unquote('%ff', encoding=None) == '\ufffd'
assert parse_qsl(b'a=%ff', encoding='nope') == [(b'a', b'\xff')]
# literal characters are kept; only the escapes go through the codec
assert parse_qsl('a=\u00e9%c3%a9') == [('a', '\u00e9\u00e9')]
assert parse_qsl('a=\u00e9%e9', encoding='ascii') == [('a', '\u00e9\ufffd')]
assert parse_qs('a=1&b=2&a=3&c=4') == {'a': ['1', '3'], 'b': ['2'], 'c': ['4']}
