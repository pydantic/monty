# === Basic f-strings ===
assert f'hello' == 'hello'
assert f'' == ''

# === Simple interpolation ===
x = 'world'
assert f'hello {x}' == 'hello world'

# multiple interpolations
a = 1
b = 2
assert f'{a} + {b} = {a + b}' == '1 + 2 = 3'

# expression in f-string
assert f'{1 + 2 + 3}' == '6'

# === Value types ===
# list value
x = [1, 2, 3]
assert f'list: {x}' == 'list: [1, 2, 3]'

# bool value
x = True
assert f'value: {x}' == 'value: True'

# int value
assert f'{42}' == '42'

# float value
assert f'{3.14}' == '3.14'

# None value
assert f'{None}' == 'None'

# === Conversion flags (!s, !r, !a) ===
# conversion !s (str)
assert f'{42!s}' == '42'

# conversion !r (repr)
assert f'{"hello"!r}' == "'hello'"

# conversion !r on int (should be same as str for int)
assert f'{42!r}' == '42'

# conversion !r on list
assert f'{[1, 2]!r}' == '[1, 2]'

# conversion !s on string (no quotes)
assert f'{"hello"!s}' == 'hello'

# conversion !a (ascii) - escapes non-ASCII characters
assert f'{"café"!a}' == "'caf\\xe9'"
assert f'{"hello"!a}' == "'hello'"
assert f'{"日本"!a}' == "'\\u65e5\\u672c'"

# === String padding and alignment ===
# format spec: width (left-aligned by default for strings)
assert f'{"hi":10}' == 'hi        '

# format spec: left align
assert f'{"hi":<10}' == 'hi        '

# format spec: right align
assert f'{"hi":>10}' == '        hi'

# format spec: center align
assert f'{"hi":^10}' == '    hi    '

# center align with odd padding
assert f'{"zip":^6}' == ' zip  '

# format spec: fill character
assert f'{"hi":*>10}' == '********hi'
assert f'{"hi":_<10}' == 'hi________'
assert f'{"hi":*^10}' == '****hi****'

# string truncation with precision
assert f'{"xylophone":.5}' == 'xylop'
assert f'{"xylophone":10.5}' == 'xylop     '

# === Integer formatting ===
# basic integer
assert f'{42}' == '42'

# integer with :d type
assert f'{42:d}' == '42'

# integer padding
assert f'{42:4d}' == '  42'
assert f'{42:04d}' == '0042'

# integer with sign
assert f'{42:+d}' == '+42'
assert f'{42: d}' == ' 42'
assert f'{-42:+d}' == '-42'
assert f'{-42: d}' == '-42'

# sign-aware padding
assert f'{-23:=5d}' == '-  23'

# i64::MIN: formatting must not overflow when taking abs of the minimum int
assert f'{-9223372036854775808:d}' == '-9223372036854775808'
assert f'{-9223372036854775808:+d}' == '-9223372036854775808'
assert f'{-9223372036854775808:=22d}' == '-  9223372036854775808'

# integer fill character with alignment
assert f'{42:*>10d}' == '********42'
assert f'{42:*<10d}' == '42********'
assert f'{42:*^10d}' == '****42****'

# === Integer non-decimal bases ===
# binary
assert f'{10:b}' == '1010'
assert f'{-10:b}' == '-1010'
assert f'{0:b}' == '0'

# octal
assert f'{8:o}' == '10'
assert f'{-8:o}' == '-10'

# hexadecimal (lower and upper)
assert f'{255:x}' == 'ff'
assert f'{-255:x}' == '-ff'
assert f'{255:X}' == 'FF'

# Uppercase `X` uppercases only the hex digits and the `0x` prefix (`0X`), NOT an
# alphabetic fill char — the fill must stay as written.
assert f'{180:a>8X}' == 'aaaaaaB4'
assert f'{0xABC:f>8X}' == 'fffffABC'
assert f'{255:#X}' == '0XFF'
assert f'{255:b>#8X}' == 'bbbb0XFF'
# Same rules for big integers (LongInt path).
assert f'{2**70:a>25X}' == 'aaaaaaa400000000000000000'
assert f'{2**68 + 0xAB:#X}' == '0X1000000000000000AB'
assert f'{2**68 + 0xAB:#x}' == '0x1000000000000000ab'

# === Sign-aware (`=`) padding applies to every numeric format, not just :d/:f ===
# Previously pad_string's SignAware arm fell through, so width was silently
# dropped for hex/oct/bin/exponential/general/percent.
assert f'{255:=10x}' == '        ff'
assert f'{-255:=10x}' == '-       ff'
assert f'{8:=8b}' == '    1000'
assert f'{8:=8o}' == '      10'
assert f'{3.14:=10g}' == '      3.14'
assert f'{-3.14:=10g}' == '-     3.14'
assert f'{0.5:=12.2%}' == '      50.00%'
# format_char has no sign; CPython accepts `=` here and degrades to right-align.
assert f'{65:=10c}' == '         A'

# === Sign prefix (`+`, ` `) applies to non-decimal integer bases too ===
# format_int_base previously ignored spec.sign and only emitted '-' for negatives.
assert f'{255:+x}' == '+ff'
assert f'{255: x}' == ' ff'
assert f'{8:+b}' == '+1000'
assert f'{-255:X}' == '-FF'

# === Integer as Unicode character (:c) ===
assert f'{65:c}' == 'A'
assert f'{0x4E2D:c}' == '中'

# === Bool with format spec ===
# bool is a subclass of int, so :d works
assert f'{True:d}' == '1'
assert f'{False:d}' == '0'
assert f'{True:04d}' == '0001'

# === Float formatting ===
# basic float
assert f'{3.14159}' == '3.14159'

# float with :f type
assert f'{3.141592653589793:f}' == '3.141593'

# float precision
assert f'{3.141592653589793:.2f}' == '3.14'
assert f'{3.141592653589793:.4f}' == '3.1416'

# float width and precision
assert f'{3.141592653589793:06.2f}' == '003.14'
assert f'{3.141592653589793:10.2f}' == '      3.14'

# float with sign
assert f'{3.14:+.2f}' == '+3.14'
assert f'{-3.14:+.2f}' == '-3.14'
assert f'{3.14:-.2f}' == '3.14'
assert f'{-3.14:-.2f}' == '-3.14'

# exponential notation
assert f'{1234.5678:e}' == '1.234568e+03'
assert f'{1234.5678:E}' == '1.234568E+03'
assert f'{1234.5678:.2e}' == '1.23e+03'
assert f'{0.00012345:.2e}' == '1.23e-04'

# general format (g/G) - uses exponential for very large/small numbers
assert f'{1.5:g}' == '1.5'
assert f'{1.500:g}' == '1.5'
assert f'{1234567890:g}' == '1.23457e+09'

# percentage
assert f'{0.25:%}' == '25.000000%'
assert f'{0.25:.1%}' == '25.0%'
assert f'{0.125:.0%}' == '12%'

# zero precision rounds (banker's/half-even style per Python)
assert f'{3.7:.0f}' == '4'
assert f'{3.4:.0f}' == '3'
assert f'{1234.5:.0e}' == '1e+03'

# uppercase exponential
assert f'{1234.5:E}' == '1.234500E+03'

# float fill character with alignment + precision
assert f'{3.14:*>10.2f}' == '******3.14'
assert f'{3.14:*<10.2f}' == '3.14******'
assert f'{3.14:*^10.2f}' == '***3.14***'

# large and small magnitude exponents
assert f'{1e100:.3e}' == '1.000e+100'
assert f'{1e-100:.3e}' == '1.000e-100'

# high precision reveals f64 representation
assert f'{0.1:.20f}' == '0.10000000000000000555'

# === Large dynamic precision ===
# Precision > u16::MAX (65535) must not overflow Rust's `format!` precision
# argument. Each of these exercises a different internal format code path.
assert f'{1:.{10**6}f}' == '1.' + '0' * 10**6
assert f'{1:.{10**6}e}' == '1.' + '0' * 10**6 + 'e+00'
assert f'{1:.{10**6}E}' == '1.' + '0' * 10**6 + 'E+00'
assert f'{0.5:.{10**6}%}' == '50.' + '0' * 10**6 + '%'
# :g strips trailing zeros, so the visible result is short, but the
# underlying format call still uses the full precision internally.
assert f'{1.5:.{10**6}g}' == '1.5'
assert f'{1e-10:.{10**6}g}' == '1.0000000000000000364321973154977415791655470655996396089904010295867919921875e-10'

# === Large static width/precision ===
# Static format specs are parsed at parse time and packed into a compact
# bytecode constant; values around the previous u16 boundary must still
# round-trip correctly.
assert len(f'{1.5:.65535f}') == 65537
assert len(f'{1.5:.65536f}') == 65538
assert len(f'{42:65536d}') == 65536

# Specs whose width or precision exceed the compact bytecode encoding
# (MAX_ENCODED_WIDTH = 2**20 - 1, MAX_ENCODED_PRECISION = 2**21 - 2)
# must still compile — the parser falls back to a dynamic spec so the
# VM re-parses at runtime.
assert len(f'{42:1048576d}') == 1048576
assert len(f'{1.5:.2097151f}') == 2097153

# Fill characters above Latin-1 (codepoint > 0xFF) don't fit the 8-bit
# fill slot of the compact encoding either — they must also round-trip
# through the dynamic-spec fallback rather than corrupting the encoded form.
assert f'{"hi":日^10}' == '日日日日hi日日日日'
assert f'{42:🐍>5d}' == '🐍🐍🐍42'

# === Integer with float format types ===
# Python allows formatting integers with float types
assert f'{42:f}' == '42.000000'
assert f'{42:.2f}' == '42.00'
assert f'{42:.2e}' == '4.20e+01'
assert f'{1234:g}' == '1234'
assert f'{5:%}' == '500.000000%'

# === Negative zero preserves sign ===
assert f'{-0.0}' == '-0.0'
assert f'{-0.0:f}' == '-0.000000'
assert f'{-0.0:+.2f}' == '-0.00'

# === The `z` flag coerces negative zero to positive zero ===
# Coercion happens AFTER rounding to the target precision: a negative value that
# rounds to zero becomes +0, but one that rounds to a nonzero value keeps its sign.
assert f'{-0.0:z}' == '0.0'
assert f'{-0.0:z.2f}' == '0.00'
assert f'{-0.001:z.1f}' == '0.0'
assert f'{-0.001:z.3f}' == '-0.001'
assert f'{-0.04:z.1f}' == '0.0'
assert f'{-0.05:z.1f}' == '-0.1'
assert f'{-0.49:z.0f}' == '0'
assert f'{-0.5:z.0f}' == '0'
assert f'{-0.0:ze}' == '0.000000e+00'
assert f'{-0.0:zg}' == '0'
assert f'{-0.0:z%}' == '0.000000%'
assert f'{0.0:z.2f}' == '0.00'
assert f'{3.14:z.1f}' == '3.1'
assert f'{-3.14:z.1f}' == '-3.1'
assert f'{-0.0:+z.1f}' == '+0.0'
assert f'{-0.0:z010.1f}' == '00000000.0'
assert f'{0:zf}' == '0.000000'
assert f'{True:zf}' == '1.000000'
# `z` in fill position (followed by an align char) is a fill char, not the flag.
assert f'{-0.0:z>8.1f}' == 'zzzz-0.0'


# `z` is only valid for floating-point presentations.
def _z_err(fn):
    try:
        fn()
        assert False, 'expected ValueError'
    except ValueError as exc:
        return str(exc)


assert _z_err(lambda: f'{-5:z}') == 'Negative zero coercion (z) not allowed in integer format specifier'
assert _z_err(lambda: f'{-5:zx}') == 'Negative zero coercion (z) not allowed in integer format specifier'
assert _z_err(lambda: f'{True:z}') == 'Negative zero coercion (z) not allowed in integer format specifier'
assert _z_err(lambda: f'{"x":z}') == 'Negative zero coercion (z) not allowed in string format specifier'
assert _z_err(lambda: f'{5:z#}') == 'Negative zero coercion (z) not allowed in integer format specifier'
assert _z_err(lambda: f'{"x":z#}') == 'Negative zero coercion (z) not allowed in string format specifier'
# Precedence: a bad type code or grouping error still wins over the z error.
assert _z_err(lambda: f'{5:z.2}') == 'Precision not allowed in integer format specifier'

# === Infinity formatting across format codes ===
# inf bypasses precision/width-pad zero rules and renders as 'inf'
assert f'{float("inf"):f}' == 'inf'
assert f'{float("inf"):e}' == 'inf'
assert f'{float("inf"):.3f}' == 'inf'
assert f'{float("inf"):+f}' == '+inf'
assert f'{float("-inf"):f}' == '-inf'

# === Nested format specs ===
width = 10
assert f'{"hi":{width}}' == 'hi        '

# nested alignment and width
align = '^'
assert f'{"test":{align}{width}}' == '   test   '

width, prec = 10, 3
assert f'{3.14159:{width}.{prec}f}' == '     3.142'

# nested precision
prec = 3
assert f'{"xylophone":.{prec}}' == 'xyl'


# === f-string in function ===
def greet(name):
    return f'Hello, {name}!'


assert greet('World') == 'Hello, World!'


# function returning formatted value
def format_num(n, w):
    return f'{n:>{w}}'


assert format_num('x', 5) == '    x'

# === Escaping ===
# double braces to escape
assert f'{{}}' == '{}'
assert f'{{x}}' == '{x}'
assert f'{{{42}}}' == '{42}'

# === Complex expressions ===
# TODO: method call on literal - parser doesn't support this yet
# assert f'{"hello".upper()}' == 'HELLO', 'method call on literal'

# TODO: method call on variable - str.upper() not implemented yet
# s = 'hello'
# assert f'{s.upper()}' == 'HELLO', 'method call on variable'

# subscript in f-string
lst = [10, 20, 30]
assert f'{lst[1]}' == '20'

# dict lookup
d = {'a': 1, 'b': 2}
assert f'{d["a"]}' == '1'

# TODO: conditional expression - parser doesn't support IfExp yet
# x = 5
# assert f'{x if x > 0 else -x}' == '5', 'conditional positive'
# x = -5
# assert f'{-x if x < 0 else x}' == '5', 'conditional negative'

# === String concatenation ===
name = 'world'
# regular string + f-string (implicit concatenation)
assert f'hello {name}' == 'hello world'

# === Empty interpolation expression ===
# (this should be a syntax error, but test current behavior)
# assert f'{}' would be syntax error

# === Whitespace in format spec ===
# no extra whitespace handling needed, width handles it
assert f'{"x":5}' == 'x    '

# === Empty format spec with various types ===
# trailing `:` with no spec behaves like no spec
assert f'{42:}' == '42'
assert f'{3.14:}' == '3.14'
assert f'{"hi":}' == 'hi'

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
assert f'{42!r:s}' == '42'

# === Conversion flag + spec: the spec is validated as a *string* spec ===
# `!s`/`!r`/`!a` convert to a string first, so flags that are illegal for text
# must be rejected exactly as they are for a real string value — and the value
# and its converted form must raise the *same* error. Valid string flags work:
assert f'{123!s:05}' == '12300'
assert f'{123!r:>6}' == '   123'
assert f'{3.14159!r:.4}' == '3.14'


# Illegal-for-text flags raise the same ValueError as on a bare string.
def _conv_err(fn):
    try:
        fn()
        assert False, 'expected ValueError'
    except ValueError as exc:
        return str(exc)


assert _conv_err(lambda: f'{123!s:#}') == 'Alternate form (#) not allowed in string format specifier'
assert _conv_err(lambda: f'{123!r:,}') == "Cannot specify ',' with 's'."
assert _conv_err(lambda: f'{123!r:_}') == "Cannot specify '_' with 's'."
assert _conv_err(lambda: f'{123!s:+}') == 'Sign not allowed in string format specifier'
assert _conv_err(lambda: f'{123!s: }') == 'Space not allowed in string format specifier'
assert _conv_err(lambda: f'{123!s:=}') == "'=' alignment not allowed in string format specifier"
assert _conv_err(lambda: f'{123!r:#x}') == "Unknown format code 'x' for object of type 'str'"
assert _conv_err(lambda: f'{123!s:.2f}') == "Unknown format code 'f' for object of type 'str'"

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
assert f'{x:05d}' == '-0042'

# === Debug/self-documenting expressions (=) ===
a = 42
assert f'{a=}' == 'a=42'
assert f'{a = }' == 'a = 42'
name = 'test'
assert f'{name=}' == "name='test'"
assert f'{name = }' == "name = 'test'"
assert f'{name=!s}' == 'name=test'
assert f'{name=!r}' == "name='test'"
assert f'{1+1=}' == '1+1=2'
# a format spec applies to the *value*, not the (default-repr) string — the
# implicit repr only kicks in when there is no spec and no conversion
_v = 6.28318
assert f'{_v=:.3f}' == '_v=6.283'
assert f'{_v=:>10.2f}' == '_v=      6.28'
assert f'{_v=!r:>12}' == '_v=     6.28318'
assert f'{_v=}' == '_v=6.28318'
# an *explicit empty* spec (`{x=:}`) formats with str, NOT the repr default —
# `format(x, "")` equals `str(x)`, so the colon disables the debug repr default
assert f'{name=:}' == 'name=test'
assert f'{a=:}' == 'a=42'
_b = True
assert f'{_b=:}' == '_b=True'
assert f'{name = :}' == 'name = test'
assert f'{name=!r:}' == "name='test'"

# === Comma thousands separator ===
assert f'{1234567:,}' == '1,234,567'
assert f'{1234:,}' == '1,234'
assert f'{12:,}' == '12'
assert f'{0:,}' == '0'
assert f'{-1234567:,}' == '-1,234,567'
assert f'{1234567:,d}' == '1,234,567'
assert f'{1234567.891:,f}' == '1,234,567.891000'
assert f'{1234567.891:,.2f}' == '1,234,567.89'
assert f'{-1234567.891:,.2f}' == '-1,234,567.89'
assert f'{1234567.891:,e}' == '1.234568e+06'
assert f'{1234.5:,g}' == '1,234.5'
assert f'{12.3456:,%}' == '1,234.560000%'
assert f'{1234:+,}' == '+1,234'
assert f'{-1234:+,}' == '-1,234'
assert f'{1234: ,}' == ' 1,234'

# === Underscore thousands separator ===
assert f'{1234567:_}' == '1_234_567'
assert f'{1234567:_d}' == '1_234_567'
assert f'{1234567.891:_.2f}' == '1_234_567.89'
# Underscore groups binary/octal/hex in fours
assert f'{255:_b}' == '1111_1111'
assert f'{0xABCDEF:_x}' == 'ab_cdef'
assert f'{0xABCDEF:_X}' == 'AB_CDEF'
assert f'{0o12345670:_o}' == '1234_5670'

# === Grouping with zero-padding (padding is itself grouped) ===
assert f'{1234567:012,d}' == '0,001,234,567'
assert f'{1234:010,}' == '00,001,234'
assert f'{-1234:010,}' == '-0,001,234'
assert f'{1234:08,}' == '0,001,234'
assert f'{1234:07,}' == '001,234'
assert f'{1234:05,}' == '1,234'
assert f'{1234567.891:020,.2f}' == '0,000,001,234,567.89'
assert f'{255:010_b}' == '0_1111_1111'

# === Grouping with explicit alignment (fill is not grouped) ===
assert f'{1234:=10,}' == '     1,234'
assert f'{1234:=+10,}' == '+    1,234'
assert f'{1234:>12,}' == '       1,234'
assert f'{1234:*>12,}' == '*******1,234'

# === Alternate form (#) on integer bases adds the 0b/0o/0x prefix ===
assert f'{255:#x}' == '0xff'
assert f'{255:#X}' == '0XFF'
assert f'{5:#b}' == '0b101'
assert f'{8:#o}' == '0o10'
assert f'{-255:#x}' == '-0xff'
assert f'{0:#x}' == '0x0'
assert f'{255:#d}' == '255'
# prefix counts toward width and sits before zero padding
assert f'{255:#10x}' == '      0xff'
assert f'{255:#010x}' == '0x000000ff'
assert f'{-255:#010x}' == '-0x00000ff'
assert f'{255:+#x}' == '+0xff'
# prefix with alignment / grouping
assert f'{255:<#10x}' == '0xff      '
assert f'{255:^#10x}' == '   0xff   '
assert f'{255:=#10x}' == '0x      ff'
assert f'{255:*=#10x}' == '0x******ff'
assert f'{0xABCDEF:#_x}' == '0xab_cdef'
assert f'{0xABCDEF:#010_x}' == '0x0ab_cdef'

# === Alternate form (#) on floats forces a decimal point ===
assert f'{1.0:#.0f}' == '1.'
assert f'{0.0:#.0f}' == '0.'
assert f'{1.0:#.0e}' == '1.e+00'
assert f'{3.14:+#.2f}' == '+3.14'
assert f'{0.5:#.0%}' == '50.%'
# explicit g/G keeps trailing zeros under #
assert f'{1.0:#g}' == '1.00000'
assert f'{100.0:#g}' == '100.000'
assert f'{1.5:#.3g}' == '1.50'
assert f'{1234.0:#.4g}' == '1234.'
# default float presentation: # is a no-op (shortest repr already has a point)
assert f'{3.14:#}' == '3.14'
# `#g`/`#G`/type-less keep every trailing zero, so the digit count scales with
# precision past Rust's formatter cap (regression: it used to silently truncate
# to ~65k digits). Verify the count is exact, not clamped.
assert f'{1.0:#.10g}' == '1.000000000'
assert f'{1.0:#.10G}' == '1.000000000'
assert f'{123456.0:#.10}' == '123456.0000'
assert len(f'{1.0:#.70000g}') == 70001
assert len(f'{1.0:#.70000}') == 70001

# === inf / nan: case follows the presentation, never `.0` ===
_inf = float('inf')
_nan = float('nan')
assert f'{_inf}' == 'inf'
assert f'{-_inf}' == '-inf'
assert f'{_nan}' == 'nan'
assert str(_inf) == 'inf' and repr(_nan) == 'nan', 'inf/nan str/repr'
assert f'{_inf:f}' == 'inf' and f'{_inf:F}' == 'INF', 'inf f vs F case'
assert f'{_nan:e}' == 'nan' and f'{_nan:E}' == 'NAN', 'nan e vs E case'
assert f'{_inf:g}' == 'inf' and f'{_inf:G}' == 'INF', 'inf g vs G case'
assert f'{_nan:%}' == 'nan%'
assert f'{_inf:+.2f}' == '+inf'
# grouping/zero-pad on a non-finite value must not panic and ignores the comma
assert f'{_inf:08,}' == '00000inf'
assert f'{-_inf:+020,}' == '-0000000000000000inf'
assert f'{_nan:,}' == 'nan'

# === float repr uses scientific notation past CPython's thresholds ===
assert f'{1e16}' == '1e+16'
assert f'{1e15}' == '1000000000000000.0'
assert f'{1e-5}' == '1e-05'
assert f'{1e-4}' == '0.0001'
assert f'{1.2345678901234568e16}' == '1.2345678901234568e+16'
assert f'{1e100}' == '1e+100'

# === type-less float spec uses repr digits, not g (precision-6) ===
assert f'{1234567.0:}' == '1234567.0'
assert f'{1234567.0:>12}' == '   1234567.0'
assert f'{1234.5678:+,}' == '+1,234.5678'

# === bool is an int subclass under a (non-empty) format spec ===
assert f'{True}' == 'True' and f'{True:}' == 'True', 'bare bool is the word'
assert f'{True: }' == ' 1'
assert f'{False:05}' == '00000'
assert f'{True:#06x}' == '0x0001'
assert f'{True:.2f}' == '1.00'

# === big integers honour the full mini-language ===
_big = 2**70
assert f'{_big:,}' == '1,180,591,620,717,411,303,424'
assert f'{_big:+}' == '+1180591620717411303424'
assert f'{_big:#x}' == '0x400000000000000000'
assert f'{_big:#o}' == '0o200000000000000000000000'
assert f'{-(2**63):>25}' == '     -9223372036854775808'
assert f'{2**63:.3e}' == '9.223e+18'

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
assert f'{-42:<05}' == '-4200'
assert f'{42:^05}' == '04200'
assert f'{42:>05}' == '00042'
assert f'{42:*<05}' == '42***'
assert f'{-42:05}' == '-0042'
assert f'{"hi":05}' == 'hi000'

# === `c` is a numeric presentation: right-aligned by default ===
assert f'{65:5c}' == '    A'
assert f'{65:<5c}' == 'A    '
assert f'{65:05c}' == '0000A'

# === `n` (locale number): like `d` for int, `g` for float (C locale here) ===
assert f'{1234567:n}' == '1234567'
assert f'{1234567.0:n}' == '1.23457e+06'
assert f'{-42:+n}' == '-42'
assert f'{2**70:n}' == '1180591620717411303424'
assert f'{True:n}' == '1'
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
assert f'{1234.5678:.6_f}' == '1234.567_800'
assert f'{1234567.89:,.4_f}' == '1,234,567.890_0'
assert f'{12345.678:._f}' == '12345.678_000'
assert f'{1234567.891:,._f}' == '1,234,567.891_000'

# === Type-less float with an explicit precision (g-like, one exp earlier) ===
assert f'{100.0:.3}' == '1e+02'
assert f'{1.0:.0}' == '1e+00'
assert f'{1234.5678:.6}' == '1234.57'
assert f'{9.99:.1g}' == '1e+01'

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
