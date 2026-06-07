# === Basic f-strings ===
assert f'hello' == 'hello', 'basic f-string'
assert f'' == '', 'empty f-string'

# === Simple interpolation ===
x = 'world'
assert f'hello {x}' == 'hello world', 'simple interpolation'

# multiple interpolations
a = 1
b = 2
assert f'{a} + {b} = {a + b}' == '1 + 2 = 3', 'multiple interpolations'

# expression in f-string
assert f'{1 + 2 + 3}' == '6', 'expression'

# === Value types ===
# list value
x = [1, 2, 3]
assert f'list: {x}' == 'list: [1, 2, 3]', 'list value'

# bool value
x = True
assert f'value: {x}' == 'value: True', 'bool value'

# int value
assert f'{42}' == '42', 'int value'

# float value
assert f'{3.14}' == '3.14', 'float value'

# None value
assert f'{None}' == 'None', 'None value'

# === Conversion flags (!s, !r, !a) ===
# conversion !s (str)
assert f'{42!s}' == '42', 'conversion !s'

# conversion !r (repr)
assert f'{"hello"!r}' == "'hello'", 'conversion !r'

# conversion !r on int (should be same as str for int)
assert f'{42!r}' == '42', 'conversion !r on int'

# conversion !r on list
assert f'{[1, 2]!r}' == '[1, 2]', 'conversion !r on list'

# conversion !s on string (no quotes)
assert f'{"hello"!s}' == 'hello', 'conversion !s on string'

# conversion !a (ascii) - escapes non-ASCII characters
assert f'{"café"!a}' == "'caf\\xe9'", 'conversion !a'
assert f'{"hello"!a}' == "'hello'", 'conversion !a ascii only'
assert f'{"日本"!a}' == "'\\u65e5\\u672c'", 'conversion !a unicode'

# === String padding and alignment ===
# format spec: width (left-aligned by default for strings)
assert f'{"hi":10}' == 'hi        ', 'format width'

# format spec: left align
assert f'{"hi":<10}' == 'hi        ', 'format left align'

# format spec: right align
assert f'{"hi":>10}' == '        hi', 'format right align'

# format spec: center align
assert f'{"hi":^10}' == '    hi    ', 'format center align'

# center align with odd padding
assert f'{"zip":^6}' == ' zip  ', 'format center align odd'

# format spec: fill character
assert f'{"hi":*>10}' == '********hi', 'format fill right'
assert f'{"hi":_<10}' == 'hi________', 'format fill left'
assert f'{"hi":*^10}' == '****hi****', 'format fill center'

# string truncation with precision
assert f'{"xylophone":.5}' == 'xylop', 'string truncation'
assert f'{"xylophone":10.5}' == 'xylop     ', 'string truncation with width'

# === Integer formatting ===
# basic integer
assert f'{42}' == '42', 'basic integer'

# integer with :d type
assert f'{42:d}' == '42', 'integer :d'

# integer padding
assert f'{42:4d}' == '  42', 'integer padding'
assert f'{42:04d}' == '0042', 'integer zero padding'

# integer with sign
assert f'{42:+d}' == '+42', 'integer positive sign'
assert f'{42: d}' == ' 42', 'integer space for positive'
assert f'{-42:+d}' == '-42', 'integer negative with sign'
assert f'{-42: d}' == '-42', 'integer negative space'

# sign-aware padding
assert f'{-23:=5d}' == '-  23', 'sign-aware padding'

# i64::MIN: formatting must not overflow when taking abs of the minimum int
assert f'{-9223372036854775808:d}' == '-9223372036854775808', 'i64 min :d'
assert f'{-9223372036854775808:+d}' == '-9223372036854775808', 'i64 min with sign'
assert f'{-9223372036854775808:=22d}' == '-  9223372036854775808', 'i64 min sign-aware padding'

# integer fill character with alignment
assert f'{42:*>10d}' == '********42', 'int fill right'
assert f'{42:*<10d}' == '42********', 'int fill left'
assert f'{42:*^10d}' == '****42****', 'int fill center'

# === Integer non-decimal bases ===
# binary
assert f'{10:b}' == '1010', 'binary positive'
assert f'{-10:b}' == '-1010', 'binary negative'
assert f'{0:b}' == '0', 'binary zero'

# octal
assert f'{8:o}' == '10', 'octal positive'
assert f'{-8:o}' == '-10', 'octal negative'

# hexadecimal (lower and upper)
assert f'{255:x}' == 'ff', 'hex lowercase'
assert f'{-255:x}' == '-ff', 'hex lowercase negative'
assert f'{255:X}' == 'FF', 'hex uppercase'

# Uppercase `X` uppercases only the hex digits and the `0x` prefix (`0X`), NOT an
# alphabetic fill char — the fill must stay as written.
assert f'{180:a>8X}' == 'aaaaaaB4', 'upper hex keeps alpha fill lowercase'
assert f'{0xABC:f>8X}' == 'fffffABC', 'upper hex with f fill keeps fill lowercase'
assert f'{255:#X}' == '0XFF', 'upper hex alternate prefix is 0X'
assert f'{255:b>#8X}' == 'bbbb0XFF', 'upper hex alternate + alpha fill'
# Same rules for big integers (LongInt path).
assert f'{2**70:a>25X}' == 'aaaaaaa400000000000000000', 'big int upper hex alpha fill'
assert f'{2**68 + 0xAB:#X}' == '0X1000000000000000AB', 'big int upper hex alternate prefix'
assert f'{2**68 + 0xAB:#x}' == '0x1000000000000000ab', 'big int lower hex alternate prefix'

# === Sign-aware (`=`) padding applies to every numeric format, not just :d/:f ===
# Previously pad_string's SignAware arm fell through, so width was silently
# dropped for hex/oct/bin/exponential/general/percent.
assert f'{255:=10x}' == '        ff', 'sign-aware width on :x positive'
assert f'{-255:=10x}' == '-       ff', 'sign-aware width on :x negative'
assert f'{8:=8b}' == '    1000', 'sign-aware width on :b'
assert f'{8:=8o}' == '      10', 'sign-aware width on :o'
assert f'{3.14:=10g}' == '      3.14', 'sign-aware width on :g'
assert f'{-3.14:=10g}' == '-     3.14', 'sign-aware width on :g negative'
assert f'{0.5:=12.2%}' == '      50.00%', 'sign-aware width on :%'
# format_char has no sign; CPython accepts `=` here and degrades to right-align.
assert f'{65:=10c}' == '         A', 'sign-aware width on :c (no sign -> right-align)'

# === Sign prefix (`+`, ` `) applies to non-decimal integer bases too ===
# format_int_base previously ignored spec.sign and only emitted '-' for negatives.
assert f'{255:+x}' == '+ff', 'plus sign on :x positive'
assert f'{255: x}' == ' ff', 'space sign on :x positive'
assert f'{8:+b}' == '+1000', 'plus sign on :b positive'
assert f'{-255:X}' == '-FF', 'hex uppercase negative'

# === Integer as Unicode character (:c) ===
assert f'{65:c}' == 'A', 'char ascii'
assert f'{0x4E2D:c}' == '中', 'char BMP unicode'

# === Bool with format spec ===
# bool is a subclass of int, so :d works
assert f'{True:d}' == '1', 'bool True as int'
assert f'{False:d}' == '0', 'bool False as int'
assert f'{True:04d}' == '0001', 'bool with zero-pad'

# === Float formatting ===
# basic float
assert f'{3.14159}' == '3.14159', 'basic float'

# float with :f type
assert f'{3.141592653589793:f}' == '3.141593', 'float :f'

# float precision
assert f'{3.141592653589793:.2f}' == '3.14', 'float precision'
assert f'{3.141592653589793:.4f}' == '3.1416', 'float precision 4'

# float width and precision
assert f'{3.141592653589793:06.2f}' == '003.14', 'float zero pad with precision'
assert f'{3.141592653589793:10.2f}' == '      3.14', 'float width with precision'

# float with sign
assert f'{3.14:+.2f}' == '+3.14', 'float positive sign'
assert f'{-3.14:+.2f}' == '-3.14', 'float negative with sign'
assert f'{3.14:-.2f}' == '3.14', 'float explicit minus sign'
assert f'{-3.14:-.2f}' == '-3.14', 'float explicit minus sign negative'

# exponential notation
assert f'{1234.5678:e}' == '1.234568e+03', 'exponential lowercase'
assert f'{1234.5678:E}' == '1.234568E+03', 'exponential uppercase'
assert f'{1234.5678:.2e}' == '1.23e+03', 'exponential with precision'
assert f'{0.00012345:.2e}' == '1.23e-04', 'exponential small number'

# general format (g/G) - uses exponential for very large/small numbers
assert f'{1.5:g}' == '1.5', 'general format simple'
assert f'{1.500:g}' == '1.5', 'general format strips trailing zeros'
assert f'{1234567890:g}' == '1.23457e+09', 'general format large number'

# percentage
assert f'{0.25:%}' == '25.000000%', 'percentage default precision'
assert f'{0.25:.1%}' == '25.0%', 'percentage with precision'
assert f'{0.125:.0%}' == '12%', 'percentage zero precision'

# zero precision rounds (banker's/half-even style per Python)
assert f'{3.7:.0f}' == '4', 'zero precision rounds up'
assert f'{3.4:.0f}' == '3', 'zero precision rounds down'
assert f'{1234.5:.0e}' == '1e+03', 'zero precision exponential'

# uppercase exponential
assert f'{1234.5:E}' == '1.234500E+03', 'uppercase E'

# float fill character with alignment + precision
assert f'{3.14:*>10.2f}' == '******3.14', 'float fill right'
assert f'{3.14:*<10.2f}' == '3.14******', 'float fill left'
assert f'{3.14:*^10.2f}' == '***3.14***', 'float fill center'

# large and small magnitude exponents
assert f'{1e100:.3e}' == '1.000e+100', 'very large exponent'
assert f'{1e-100:.3e}' == '1.000e-100', 'very small exponent'

# high precision reveals f64 representation
assert f'{0.1:.20f}' == '0.10000000000000000555', 'high precision float'

# === Large dynamic precision ===
# Precision > u16::MAX (65535) must not overflow Rust's `format!` precision
# argument. Each of these exercises a different internal format code path.
assert f'{1:.{10**6}f}' == '1.' + '0' * 10**6, 'huge precision :f'
assert f'{1:.{10**6}e}' == '1.' + '0' * 10**6 + 'e+00', 'huge precision :e'
assert f'{1:.{10**6}E}' == '1.' + '0' * 10**6 + 'E+00', 'huge precision :E'
assert f'{0.5:.{10**6}%}' == '50.' + '0' * 10**6 + '%', 'huge precision :%'
# :g strips trailing zeros, so the visible result is short, but the
# underlying format call still uses the full precision internally.
assert f'{1.5:.{10**6}g}' == '1.5', 'huge precision :g fixed branch'
assert f'{1e-10:.{10**6}g}' == '1.0000000000000000364321973154977415791655470655996396089904010295867919921875e-10', (
    'huge precision :g exponential branch'
)

# === Large static width/precision ===
# Static format specs are parsed at parse time and packed into a compact
# bytecode constant; values around the previous u16 boundary must still
# round-trip correctly.
assert len(f'{1.5:.65535f}') == 65537, 'static precision 65535'
assert len(f'{1.5:.65536f}') == 65538, 'static precision 65536'
assert len(f'{42:65536d}') == 65536, 'static width 65536'

# Specs whose width or precision exceed the compact bytecode encoding
# (MAX_ENCODED_WIDTH = 2**20 - 1, MAX_ENCODED_PRECISION = 2**21 - 2)
# must still compile — the parser falls back to a dynamic spec so the
# VM re-parses at runtime.
assert len(f'{42:1048576d}') == 1048576, 'static width past compact encoding'
assert len(f'{1.5:.2097151f}') == 2097153, 'static precision past compact encoding'

# Fill characters above Latin-1 (codepoint > 0xFF) don't fit the 8-bit
# fill slot of the compact encoding either — they must also round-trip
# through the dynamic-spec fallback rather than corrupting the encoded form.
assert f'{"hi":日^10}' == '日日日日hi日日日日', 'non-latin-1 fill char (CJK)'
assert f'{42:🐍>5d}' == '🐍🐍🐍42', 'non-latin-1 fill char (emoji)'

# === Integer with float format types ===
# Python allows formatting integers with float types
assert f'{42:f}' == '42.000000', 'int as :f'
assert f'{42:.2f}' == '42.00', 'int as :.2f'
assert f'{42:.2e}' == '4.20e+01', 'int as :.2e'
assert f'{1234:g}' == '1234', 'int as :g'
assert f'{5:%}' == '500.000000%', 'int as :%'

# === Negative zero preserves sign ===
assert f'{-0.0}' == '-0.0', 'negative zero default'
assert f'{-0.0:f}' == '-0.000000', 'negative zero :f'
assert f'{-0.0:+.2f}' == '-0.00', 'negative zero with sign'

# === The `z` flag coerces negative zero to positive zero ===
# Coercion happens AFTER rounding to the target precision: a negative value that
# rounds to zero becomes +0, but one that rounds to a nonzero value keeps its sign.
assert f'{-0.0:z}' == '0.0', 'z on bare -0.0 (default repr)'
assert f'{-0.0:z.2f}' == '0.00', 'z on -0.0 fixed'
assert f'{-0.001:z.1f}' == '0.0', 'z: -0.001 rounds to -0.0 -> +0.0'
assert f'{-0.001:z.3f}' == '-0.001', 'z: -0.001 not zero at .3f, keeps sign'
assert f'{-0.04:z.1f}' == '0.0', 'z: -0.04 rounds to zero'
assert f'{-0.05:z.1f}' == '-0.1', 'z: -0.05 rounds away from zero, keeps sign'
assert f'{-0.49:z.0f}' == '0', 'z: -0.49 rounds to 0'
assert f'{-0.5:z.0f}' == '0', "z: -0.5 banker's-rounds to 0"
assert f'{-0.0:ze}' == '0.000000e+00', 'z in scientific'
assert f'{-0.0:zg}' == '0', 'z in general format'
assert f'{-0.0:z%}' == '0.000000%', 'z in percent'
assert f'{0.0:z.2f}' == '0.00', 'z on positive zero is a no-op'
assert f'{3.14:z.1f}' == '3.1', 'z on positive nonzero is a no-op'
assert f'{-3.14:z.1f}' == '-3.1', 'z on negative nonzero keeps sign'
assert f'{-0.0:+z.1f}' == '+0.0', 'z with explicit + sign'
assert f'{-0.0:z010.1f}' == '00000000.0', 'z with zero-padding'
assert f'{0:zf}' == '0.000000', 'z on int via float type'
assert f'{True:zf}' == '1.000000', 'z on bool via float type'
# `z` in fill position (followed by an align char) is a fill char, not the flag.
assert f'{-0.0:z>8.1f}' == 'zzzz-0.0', 'z as fill is not the coercion flag'


# `z` is only valid for floating-point presentations.
def _z_err(fn):
    try:
        fn()
        assert False, 'expected ValueError'
    except ValueError as exc:
        return str(exc)


assert _z_err(lambda: f'{-5:z}') == 'Negative zero coercion (z) not allowed in integer format specifier', 'z on int'
assert _z_err(lambda: f'{-5:zx}') == 'Negative zero coercion (z) not allowed in integer format specifier', 'z on hex'
assert _z_err(lambda: f'{True:z}') == 'Negative zero coercion (z) not allowed in integer format specifier', 'z on bool'
assert _z_err(lambda: f'{"x":z}') == 'Negative zero coercion (z) not allowed in string format specifier', 'z on str'
assert _z_err(lambda: f'{5:z#}') == 'Negative zero coercion (z) not allowed in integer format specifier', (
    'z beats alternate (int)'
)
assert _z_err(lambda: f'{"x":z#}') == 'Negative zero coercion (z) not allowed in string format specifier', (
    'z beats alternate (str)'
)
# Precedence: a bad type code or grouping error still wins over the z error.
assert _z_err(lambda: f'{5:z.2}') == 'Precision not allowed in integer format specifier', 'precision-on-int beats z'

# === Infinity formatting across format codes ===
# inf bypasses precision/width-pad zero rules and renders as 'inf'
assert f'{float("inf"):f}' == 'inf', 'inf :f'
assert f'{float("inf"):e}' == 'inf', 'inf :e'
assert f'{float("inf"):.3f}' == 'inf', 'inf with precision'
assert f'{float("inf"):+f}' == '+inf', 'inf with sign'
assert f'{float("-inf"):f}' == '-inf', 'negative inf'

# === Nested format specs ===
width = 10
assert f'{"hi":{width}}' == 'hi        ', 'nested format spec width'

# nested alignment and width
align = '^'
assert f'{"test":{align}{width}}' == '   test   ', 'nested align and width'

width, prec = 10, 3
assert f'{3.14159:{width}.{prec}f}' == '     3.142', 'computed width + precision'

# nested precision
prec = 3
assert f'{"xylophone":.{prec}}' == 'xyl', 'nested precision'


# === f-string in function ===
def greet(name):
    return f'Hello, {name}!'


assert greet('World') == 'Hello, World!', 'f-string in function'


# function returning formatted value
def format_num(n, w):
    return f'{n:>{w}}'


assert format_num('x', 5) == '    x', 'f-string with params'

# === Escaping ===
# double braces to escape
assert f'{{}}' == '{}', 'escaped braces'
assert f'{{x}}' == '{x}', 'escaped braces with content'
assert f'{{{42}}}' == '{42}', 'value inside escaped braces'

# === Complex expressions ===
# TODO: method call on literal - parser doesn't support this yet
# assert f'{"hello".upper()}' == 'HELLO', 'method call on literal'

# TODO: method call on variable - str.upper() not implemented yet
# s = 'hello'
# assert f'{s.upper()}' == 'HELLO', 'method call on variable'

# subscript in f-string
lst = [10, 20, 30]
assert f'{lst[1]}' == '20', 'subscript'

# dict lookup
d = {'a': 1, 'b': 2}
assert f'{d["a"]}' == '1', 'dict lookup'

# TODO: conditional expression - parser doesn't support IfExp yet
# x = 5
# assert f'{x if x > 0 else -x}' == '5', 'conditional positive'
# x = -5
# assert f'{-x if x < 0 else x}' == '5', 'conditional negative'

# === String concatenation ===
name = 'world'
# regular string + f-string (implicit concatenation)
assert f'hello {name}' == 'hello world', 'str concat with fstring'

# === Empty interpolation expression ===
# (this should be a syntax error, but test current behavior)
# assert f'{}' would be syntax error

# === Whitespace in format spec ===
# no extra whitespace handling needed, width handles it
assert f'{"x":5}' == 'x    ', 'single char width'

# === Empty format spec with various types ===
# trailing `:` with no spec behaves like no spec
assert f'{42:}' == '42', 'empty spec int'
assert f'{3.14:}' == '3.14', 'empty spec float'
assert f'{"hi":}' == 'hi', 'empty spec string'

# === Unicode character counting in padding ===
x = 'café'
assert f'{x:_<10}' == 'café______'
assert f'{x:_>10}' == '______café'
assert f'{x:_^10}' == '___café___'
assert f'{x:_^11}' == '___café____'
assert f'{x:é<10}' == 'cafééééééé'
assert f'{x:é>10}' == 'éééééécafé'
assert f'{x:é^10}' == 'ééécaféééé'
assert f'{x:é^11}' == 'ééécafééééé'

# === Conversion flag with type spec ===
# conversion flag produces string, so 's' format should work
assert f'{42!r:s}' == '42', 'conversion with type spec'

# === Conversion flag + spec: the spec is validated as a *string* spec ===
# `!s`/`!r`/`!a` convert to a string first, so flags that are illegal for text
# must be rejected exactly as they are for a real string value — and the value
# and its converted form must raise the *same* error. Valid string flags work:
assert f'{123!s:05}' == '12300', 'conversion + zero-pad formats like a string'
assert f'{123!r:>6}' == '   123', 'conversion + width/align'
assert f'{3.14159!r:.4}' == '3.14', 'conversion + precision truncates the repr'


# Illegal-for-text flags raise the same ValueError as on a bare string.
def _conv_err(fn):
    try:
        fn()
        assert False, 'expected ValueError'
    except ValueError as exc:
        return str(exc)


assert _conv_err(lambda: f'{123!s:#}') == 'Alternate form (#) not allowed in string format specifier', (
    'alternate via !s'
)
assert _conv_err(lambda: f'{123!r:,}') == "Cannot specify ',' with 's'.", 'comma grouping via !r'
assert _conv_err(lambda: f'{123!r:_}') == "Cannot specify '_' with 's'.", 'underscore grouping via !r'
assert _conv_err(lambda: f'{123!s:+}') == 'Sign not allowed in string format specifier', 'sign via !s'
assert _conv_err(lambda: f'{123!s: }') == 'Space not allowed in string format specifier', 'space sign via !s'
assert _conv_err(lambda: f'{123!s:=}') == "'=' alignment not allowed in string format specifier", (
    'sign-aware align via !s'
)
assert _conv_err(lambda: f'{123!r:#x}') == "Unknown format code 'x' for object of type 'str'", 'type code via !r'
assert _conv_err(lambda: f'{123!s:.2f}') == "Unknown format code 'f' for object of type 'str'", 'float type via !s'

# Precedence among multiple violations matches CPython (grouping > type > sign >
# alternate > `=`), and a value formats identically with or without `!s`/`!r`.
assert (
    _conv_err(lambda: f'{"x":=#}')
    == _conv_err(lambda: f'{1!r:=#}')
    == 'Alternate form (#) not allowed in string format specifier'
), 'alternate beats = align'
assert (
    _conv_err(lambda: f'{"x":+#}') == _conv_err(lambda: f'{1!s:+#}') == 'Sign not allowed in string format specifier'
), 'sign beats alternate'
assert _conv_err(lambda: f'{"x":,x}') == _conv_err(lambda: f'{1!r:,x}') == "Cannot specify ',' with 'x'.", (
    'grouping beats type'
)

# === Zero-padding with negative numbers ===
# zero-padding should use sign-aware alignment
x = -42
assert f'{x:05d}' == '-0042', 'zero pad negative'

# === Debug/self-documenting expressions (=) ===
a = 42
assert f'{a=}' == 'a=42', 'basic debug expression'
assert f'{a = }' == 'a = 42', 'debug with spaces'
name = 'test'
assert f'{name=}' == "name='test'", 'debug uses repr for strings'
assert f'{name = }' == "name = 'test'", 'debug uses repr for strings'
assert f'{name=!s}' == 'name=test', 'debug with !s conversion'
assert f'{name=!r}' == "name='test'", 'debug with !r conversion'
assert f'{1+1=}' == '1+1=2', 'debug with expression'
# a format spec applies to the *value*, not the (default-repr) string — the
# implicit repr only kicks in when there is no spec and no conversion
_v = 6.28318
assert f'{_v=:.3f}' == '_v=6.283', 'debug spec applies to the value, not repr'
assert f'{_v=:>10.2f}' == '_v=      6.28', 'debug spec with width applies to value'
assert f'{_v=!r:>12}' == '_v=     6.28318', 'debug !r conversion then string spec'
assert f'{_v=}' == '_v=6.28318', 'debug with no spec still uses repr'
# an *explicit empty* spec (`{x=:}`) formats with str, NOT the repr default —
# `format(x, "")` equals `str(x)`, so the colon disables the debug repr default
assert f'{name=:}' == 'name=test', 'debug empty spec uses str not repr'
assert f'{a=:}' == 'a=42', 'debug empty spec on int'
_b = True
assert f'{_b=:}' == '_b=True', 'debug empty spec on bool stays True'
assert f'{name = :}' == 'name = test', 'debug empty spec with spaces uses str'
assert f'{name=!r:}' == "name='test'", 'debug empty spec with explicit !r still repr'

# === Comma thousands separator ===
assert f'{1234567:,}' == '1,234,567', 'comma groups of three'
assert f'{1234:,}' == '1,234', 'comma single group'
assert f'{12:,}' == '12', 'comma below group size'
assert f'{0:,}' == '0', 'comma zero'
assert f'{-1234567:,}' == '-1,234,567', 'comma negative'
assert f'{1234567:,d}' == '1,234,567', 'comma with explicit d'
assert f'{1234567.891:,f}' == '1,234,567.891000', 'comma float fixed'
assert f'{1234567.891:,.2f}' == '1,234,567.89', 'comma float precision'
assert f'{-1234567.891:,.2f}' == '-1,234,567.89', 'comma negative float'
assert f'{1234567.891:,e}' == '1.234568e+06', 'comma exponential (no integer groups)'
assert f'{1234.5:,g}' == '1,234.5', 'comma general fixed'
assert f'{12.3456:,%}' == '1,234.560000%', 'comma percent'
assert f'{1234:+,}' == '+1,234', 'comma with plus sign'
assert f'{-1234:+,}' == '-1,234', 'comma with plus sign negative'
assert f'{1234: ,}' == ' 1,234', 'comma with space sign'

# === Underscore thousands separator ===
assert f'{1234567:_}' == '1_234_567', 'underscore groups of three'
assert f'{1234567:_d}' == '1_234_567', 'underscore with explicit d'
assert f'{1234567.891:_.2f}' == '1_234_567.89', 'underscore float'
# Underscore groups binary/octal/hex in fours
assert f'{255:_b}' == '1111_1111', 'underscore binary groups of four'
assert f'{0xABCDEF:_x}' == 'ab_cdef', 'underscore hex'
assert f'{0xABCDEF:_X}' == 'AB_CDEF', 'underscore hex upper'
assert f'{0o12345670:_o}' == '1234_5670', 'underscore octal'

# === Grouping with zero-padding (padding is itself grouped) ===
assert f'{1234567:012,d}' == '0,001,234,567', 'comma zero-pad wider'
assert f'{1234:010,}' == '00,001,234', 'comma zero-pad'
assert f'{-1234:010,}' == '-0,001,234', 'comma zero-pad negative'
assert f'{1234:08,}' == '0,001,234', 'comma zero-pad overshoot by separator'
assert f'{1234:07,}' == '001,234', 'comma zero-pad exact'
assert f'{1234:05,}' == '1,234', 'comma zero-pad no padding needed'
assert f'{1234567.891:020,.2f}' == '0,000,001,234,567.89', 'comma zero-pad float'
assert f'{255:010_b}' == '0_1111_1111', 'underscore zero-pad binary groups of four'

# === Grouping with explicit alignment (fill is not grouped) ===
assert f'{1234:=10,}' == '     1,234', 'sign-aware fill not grouped'
assert f'{1234:=+10,}' == '+    1,234', 'sign-aware fill after sign'
assert f'{1234:>12,}' == '       1,234', 'right align grouped'
assert f'{1234:*>12,}' == '*******1,234', 'custom fill right align grouped'

# === Alternate form (#) on integer bases adds the 0b/0o/0x prefix ===
assert f'{255:#x}' == '0xff', 'hex prefix'
assert f'{255:#X}' == '0XFF', 'upper hex prefix and digits'
assert f'{5:#b}' == '0b101', 'binary prefix'
assert f'{8:#o}' == '0o10', 'octal prefix'
assert f'{-255:#x}' == '-0xff', 'negative hex prefix after sign'
assert f'{0:#x}' == '0x0', 'zero still gets prefix'
assert f'{255:#d}' == '255', 'alternate is a no-op for decimal'
# prefix counts toward width and sits before zero padding
assert f'{255:#10x}' == '      0xff', 'prefix counts toward width'
assert f'{255:#010x}' == '0x000000ff', 'zero pad goes between prefix and digits'
assert f'{-255:#010x}' == '-0x00000ff', 'zero pad after sign and prefix'
assert f'{255:+#x}' == '+0xff', 'sign before prefix'
# prefix with alignment / grouping
assert f'{255:<#10x}' == '0xff      ', 'left align with prefix'
assert f'{255:^#10x}' == '   0xff   ', 'center align with prefix'
assert f'{255:=#10x}' == '0x      ff', 'sign-aware: prefix then fill then digits'
assert f'{255:*=#10x}' == '0x******ff', 'sign-aware custom fill with prefix'
assert f'{0xABCDEF:#_x}' == '0xab_cdef', 'prefix with underscore grouping'
assert f'{0xABCDEF:#010_x}' == '0x0ab_cdef', 'prefix + zero-pad + grouping'

# === Alternate form (#) on floats forces a decimal point ===
assert f'{1.0:#.0f}' == '1.', 'force point on fixed'
assert f'{0.0:#.0f}' == '0.', 'force point on zero'
assert f'{1.0:#.0e}' == '1.e+00', 'force point on exponential'
assert f'{3.14:+#.2f}' == '+3.14', 'point already present is unchanged'
assert f'{0.5:#.0%}' == '50.%', 'force point before percent'
# explicit g/G keeps trailing zeros under #
assert f'{1.0:#g}' == '1.00000', 'g keeps trailing zeros'
assert f'{100.0:#g}' == '100.000', 'g keeps trailing zeros wider'
assert f'{1.5:#.3g}' == '1.50', 'g precision keeps zeros'
assert f'{1234.0:#.4g}' == '1234.', 'g all sig figs used, force point'
# default float presentation: # is a no-op (shortest repr already has a point)
assert f'{3.14:#}' == '3.14', 'alternate no-op on default float'
# `#g`/`#G`/type-less keep every trailing zero, so the digit count scales with
# precision past Rust's formatter cap (regression: it used to silently truncate
# to ~65k digits). Verify the count is exact, not clamped.
assert f'{1.0:#.10g}' == '1.000000000', '#g trailing zeros'
assert f'{1.0:#.10G}' == '1.000000000', '#G trailing zeros'
assert f'{123456.0:#.10}' == '123456.0000', 'type-less #g trailing zeros'
assert len(f'{1.0:#.70000g}') == 70001, '#g large precision not truncated'
assert len(f'{1.0:#.70000}') == 70001, 'type-less # large precision not truncated'

# === inf / nan: case follows the presentation, never `.0` ===
_inf = float('inf')
_nan = float('nan')
assert f'{_inf}' == 'inf', 'inf default'
assert f'{-_inf}' == '-inf', 'negative inf default'
assert f'{_nan}' == 'nan', 'nan default (lowercase, no .0)'
assert str(_inf) == 'inf' and repr(_nan) == 'nan', 'inf/nan str/repr'
assert f'{_inf:f}' == 'inf' and f'{_inf:F}' == 'INF', 'inf f vs F case'
assert f'{_nan:e}' == 'nan' and f'{_nan:E}' == 'NAN', 'nan e vs E case'
assert f'{_inf:g}' == 'inf' and f'{_inf:G}' == 'INF', 'inf g vs G case'
assert f'{_nan:%}' == 'nan%', 'nan percent stays lowercase'
assert f'{_inf:+.2f}' == '+inf', 'inf takes the sign flag'
# grouping/zero-pad on a non-finite value must not panic and ignores the comma
assert f'{_inf:08,}' == '00000inf', 'zero-pad inf ungrouped'
assert f'{-_inf:+020,}' == '-0000000000000000inf', 'negative inf zero-pad'
assert f'{_nan:,}' == 'nan', 'comma on nan is a no-op'

# === float repr uses scientific notation past CPython's thresholds ===
assert f'{1e16}' == '1e+16', 'large float switches to scientific'
assert f'{1e15}' == '1000000000000000.0', 'just below the threshold stays fixed'
assert f'{1e-5}' == '1e-05', 'small float switches to scientific'
assert f'{1e-4}' == '0.0001', 'just above the small threshold stays fixed'
assert f'{1.2345678901234568e16}' == '1.2345678901234568e+16', '17-digit float'
assert f'{1e100}' == '1e+100', 'three-digit exponent'

# === type-less float spec uses repr digits, not g (precision-6) ===
assert f'{1234567.0:}' == '1234567.0', 'empty spec keeps repr, not g'
assert f'{1234567.0:>12}' == '   1234567.0', 'type-less with width pads repr'
assert f'{1234.5678:+,}' == '+1,234.5678', 'type-less sign + grouping on repr digits'

# === bool is an int subclass under a (non-empty) format spec ===
assert f'{True}' == 'True' and f'{True:}' == 'True', 'bare bool is the word'
assert f'{True: }' == ' 1', 'bool with a directive formats numerically'
assert f'{False:05}' == '00000', 'bool zero-padded as int 0'
assert f'{True:#06x}' == '0x0001', 'bool through hex presentation'
assert f'{True:.2f}' == '1.00', 'bool through float presentation'

# === big integers honour the full mini-language ===
_big = 2**70
assert f'{_big:,}' == '1,180,591,620,717,411,303,424', 'bigint grouping'
assert f'{_big:+}' == '+1180591620717411303424', 'bigint sign'
assert f'{_big:#x}' == '0x400000000000000000', 'bigint hex with prefix'
assert f'{_big:#o}' == '0o200000000000000000000000', 'bigint octal'
assert f'{-(2**63):>25}' == '     -9223372036854775808', 'bigint width/align'
assert f'{2**63:.3e}' == '9.223e+18', 'bigint through scientific float'

# === precision is rejected on integer presentations ===
for _spec in ('.2d', '.2', '.2x', '.2b', '.0c'):
    try:
        f'{42:{_spec}}'
        assert False, f'expected precision on int spec {_spec!r} to fail'
    except ValueError as _e:
        assert str(_e) == 'Precision not allowed in integer format specifier', f'{_spec}: {_e}'

# === c rejects a sign; strings reject signs and `=` alignment ===
try:
    f'{65:+c}'
    assert False, 'expected sign with c to fail'
except ValueError as _e:
    assert str(_e) == "Sign not allowed with integer format specifier 'c'", f'sign+c: {_e}'
try:
    f'{"x":+}'
    assert False, 'expected sign on string to fail'
except ValueError as _e:
    assert str(_e) == 'Sign not allowed in string format specifier', f'sign+str: {_e}'
try:
    f'{"x": }'
    assert False, 'expected space on string to fail'
except ValueError as _e:
    assert str(_e) == 'Space not allowed in string format specifier', f'space+str: {_e}'
try:
    f'{"x":=5}'
    assert False, 'expected = alignment on string to fail'
except ValueError as _e:
    assert str(_e) == "'=' alignment not allowed in string format specifier", f'=+str: {_e}'

# === `0` flag with an explicit alignment is just a `0` fill, not sign-aware ===
assert f'{-42:<05}' == '-4200', 'explicit < with 0: 0 is fill, left-aligned'
assert f'{42:^05}' == '04200', 'explicit ^ with 0: centered, 0 fill'
assert f'{42:>05}' == '00042', 'explicit > with 0'
assert f'{42:*<05}' == '42***', 'explicit fill wins over 0'
assert f'{-42:05}' == '-0042', 'no explicit align: 0 is sign-aware'
assert f'{"hi":05}' == 'hi000', 'string 0-pad fills with 0, left default'

# === `c` is a numeric presentation: right-aligned by default ===
assert f'{65:5c}' == '    A', 'c defaults to right align'
assert f'{65:<5c}' == 'A    ', 'explicit left on c'
assert f'{65:05c}' == '0000A', 'c with 0 fill (right)'

# === `n` (locale number): like `d` for int, `g` for float (C locale here) ===
assert f'{1234567:n}' == '1234567', 'n on int = d (no grouping in C locale)'
assert f'{1234567.0:n}' == '1.23457e+06', 'n on float = g'
assert f'{-42:+n}' == '-42', 'n keeps the sign flag'
assert f'{2**70:n}' == '1180591620717411303424', 'n on a big int'
assert f'{True:n}' == '1', 'n on a bool'
# n forbids an explicit grouping and (for ints) a precision
try:
    f'{1234:,n}'
    assert False, 'expected , with n to fail'
except ValueError as _e:
    assert str(_e) == "Cannot specify ',' with 'n'.", f',n: {_e}'
try:
    f'{42:.2n}'
    assert False, 'expected precision with int n to fail'
except ValueError as _e:
    assert str(_e) == 'Precision not allowed in integer format specifier', f'.2n: {_e}'

# === Fractional grouping (Python 3.14): [.precision][grouping] ===
assert f'{1234.5678:.6_f}' == '1234.567_800', 'underscore-group the fraction'
assert f'{1234567.89:,.4_f}' == '1,234,567.890_0', 'group both integer (,) and fraction (_)'
assert f'{12345.678:._f}' == '12345.678_000', 'fraction grouping with no precision digits'
assert f'{1234567.891:,._f}' == '1,234,567.891_000', 'comma int + underscore fraction'

# === Type-less float with an explicit precision (g-like, one exp earlier) ===
assert f'{100.0:.3}' == '1e+02', 'type-less .3 goes scientific (unlike .3g)'
assert f'{1.0:.0}' == '1e+00', 'type-less .0'
assert f'{1234.5678:.6}' == '1234.57', 'type-less .6 fixed'
assert f'{9.99:.1g}' == '1e+01', 'g exponent taken after rounding (9.99 -> 10)'

# === Error precedence: an invalid type code beats the #/sign checks ===
try:
    f'{3.14:#c}'
    assert False, 'expected #c on float to fail'
except ValueError as _e:
    assert str(_e) == "Unknown format code 'c' for object of type 'float'", f'#c float: {_e}'
try:
    f'{42:#s}'
    assert False, 'expected #s on int to fail'
except ValueError as _e:
    assert str(_e) == "Unknown format code 's' for object of type 'int'", f'#s int: {_e}'
