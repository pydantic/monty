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
