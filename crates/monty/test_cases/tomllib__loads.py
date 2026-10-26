import math
import tomllib
from datetime import date, datetime, time, timedelta, timezone

# === module surface ===
assert tomllib.loads('') == {}
assert tomllib.loads('   \n\n  # just a comment\n') == {}

# === scalars ===
doc = tomllib.loads("""
str1 = "basic"
str2 = 'literal'
int1 = 42
int2 = -17
int3 = +5
int4 = 1_000_000
int5 = 0
hex1 = 0xDEADBEEF
hex2 = 0xdead_beef
oct1 = 0o755
bin1 = 0b1101_0110
flt1 = 1.0
flt2 = -0.25
flt3 = 6.626e-34
flt4 = 1e6
flt5 = 224_617.445_991_228
flt6 = 5E+22
bool1 = true
bool2 = false
inf1 = inf
inf2 = +inf
inf3 = -inf
nan1 = nan
nan2 = -nan
""")
assert doc['str1'] == 'basic'
assert doc['str2'] == 'literal'
assert doc['int1'] == 42
assert doc['int2'] == -17
assert doc['int3'] == 5
assert doc['int4'] == 1000000
assert doc['int5'] == 0
assert doc['hex1'] == 3735928559
assert doc['hex2'] == 3735928559
assert doc['oct1'] == 493
assert doc['bin1'] == 214
assert doc['flt1'] == 1.0
assert type(doc['flt1']) is float
assert doc['flt2'] == -0.25
assert doc['flt3'] == 6.626e-34
assert doc['flt4'] == 1000000.0
assert doc['flt5'] == 224617.445991228
assert doc['flt6'] == 5e22
assert doc['bool1'] is True
assert doc['bool2'] is False
assert doc['inf1'] == float('inf')
assert doc['inf2'] == float('inf')
assert doc['inf3'] == float('-inf')
assert doc['nan1'] != doc['nan1']
assert doc['nan2'] != doc['nan2']
assert math.copysign(1.0, doc['nan1']) == 1.0
assert math.copysign(1.0, doc['nan2']) == -1.0
assert repr(tomllib.loads('x = -0.0')['x']) == '-0.0'
assert repr(tomllib.loads('x = 1e400')['x']) == 'inf'
assert type(tomllib.loads('x = 1e400')['x']) is float

# === big integers ===
assert tomllib.loads('x = 9223372036854775807')['x'] == 9223372036854775807
assert tomllib.loads('x = 9223372036854775808')['x'] == 9223372036854775808
assert tomllib.loads('x = -9223372036854775808')['x'] == -9223372036854775808
assert tomllib.loads('x = -9223372036854775809')['x'] == -9223372036854775809
assert tomllib.loads('x = 0x7fff_ffff_ffff_ffff')['x'] == 9223372036854775807
assert tomllib.loads('x = 0xffff_ffff_ffff_ffff')['x'] == 18446744073709551615
assert tomllib.loads('x = 9_223_372_036_854_775_808')['x'] == 9223372036854775808
assert tomllib.loads('x = -0')['x'] == 0
assert tomllib.loads('x = 0xffffffffffffffffffff')['x'] == 1208925819614629174706175
assert tomllib.loads('x = 123456789012345678901234567890')['x'] == 123456789012345678901234567890

# === strings ===
assert tomllib.loads('x = "tab\\there"')['x'] == 'tab\there'
assert tomllib.loads('x = "a\\nb\\rc\\fd\\be"')['x'] == 'a\nb\rc\fd\be'
assert tomllib.loads('x = "quote \\" and backslash \\\\"')['x'] == 'quote " and backslash \\'
assert tomllib.loads('x = "\\u00e9\\U0001F600"')['x'] == 'é😀'
assert tomllib.loads("x = 'C:\\Users\\nodejs\\templates'")['x'] == 'C:\\Users\\nodejs\\templates'
assert tomllib.loads("x = 'no \\u escapes here'")['x'] == 'no \\u escapes here'
assert tomllib.loads('x = "unicode: ☃ é"')['x'] == 'unicode: ☃ é'
assert tomllib.loads('x = ""')['x'] == ''
assert tomllib.loads("x = ''")['x'] == ''
assert tomllib.loads('x = "tab\tinside"')['x'] == 'tab\tinside'

# multi-line basic strings
assert tomllib.loads('x = """\nRoses are red\nViolets are blue"""')['x'] == 'Roses are red\nViolets are blue'
assert tomllib.loads('x = """first line\nsecond"""')['x'] == 'first line\nsecond'
assert tomllib.loads('x = """\nThe quick brown \\\n\n\n  fox jumps over \\\n    the lazy dog."""')['x'] == (
    'The quick brown fox jumps over the lazy dog.'
)
assert tomllib.loads('x = """\\\n  trailing backslash with spaces \\   \n  done"""')['x'] == (
    'trailing backslash with spaces done'
)
assert tomllib.loads('x = """Here are two quotation marks: "". Simple enough."""')['x'] == (
    'Here are two quotation marks: "". Simple enough.'
)
assert tomllib.loads('x = """Here are three quotation marks: ""\\"."""')['x'] == (
    'Here are three quotation marks: """.'
)
assert tomllib.loads('x = """Here are fifteen quotation marks: ""\\"""\\"""\\"""\\"""\\"."""')['x'] == (
    'Here are fifteen quotation marks: """"""""""""""".'
)
assert tomllib.loads('x = """"This," she said, "is just a pointless statement.""""')['x'] == (
    '"This," she said, "is just a pointless statement."'
)
assert tomllib.loads('x = """a"""""')['x'] == 'a""'
assert tomllib.loads('x = """"""')['x'] == ''
assert tomllib.loads('x = """\n"""')['x'] == ''
assert tomllib.loads('x = """\\u0041"""')['x'] == 'A'
assert tomllib.loads('x = """\\ \n  stripped"""')['x'] == 'stripped'

# multi-line literal strings
assert (
    tomllib.loads(
        "x = '''\nThe first newline is\ntrimmed in raw strings.\n   All other whitespace\n   is preserved.\n'''"
    )['x']
    == 'The first newline is\ntrimmed in raw strings.\n   All other whitespace\n   is preserved.\n'
)
assert tomllib.loads("x = '''I [dw]on't need \\d{2} apples'''")['x'] == "I [dw]on't need \\d{2} apples"
assert (
    tomllib.loads("x = ''''That,' she said, 'is still pointless.''''")['x'] == "'That,' she said, 'is still pointless.'"
)
assert tomllib.loads("x = '''Here are two apostrophes: ''. Simple enough.'''")['x'] == (
    "Here are two apostrophes: ''. Simple enough."
)
assert tomllib.loads("x = '''\\'''")['x'] == '\\'

# \r\n is normalised to \n, inside strings too
assert tomllib.loads('a = 1\r\nb = 2\r\n') == {'a': 1, 'b': 2}
assert tomllib.loads('x = """a\r\nb"""')['x'] == 'a\nb'
assert tomllib.loads("x = '''a\r\nb'''")['x'] == 'a\nb'

# === keys ===
assert tomllib.loads('bare_key = 1\nbare-key = 2\n1234 = 3') == {'bare_key': 1, 'bare-key': 2, '1234': 3}
assert tomllib.loads('"127.0.0.1" = "value"\n"character encoding" = "value"\n"ʎǝʞ" = "value"') == {
    '127.0.0.1': 'value',
    'character encoding': 'value',
    'ʎǝʞ': 'value',
}
assert tomllib.loads("'key2' = 1\n'quoted \"value\"' = 2") == {'key2': 1, 'quoted "value"': 2}
assert tomllib.loads('"" = "blank"') == {'': 'blank'}
assert tomllib.loads("'' = 'blank'") == {'': 'blank'}
assert tomllib.loads('"\\u00e9" = 1') == {'é': 1}
assert tomllib.loads('a   =   1\nb\t=\t2') == {'a': 1, 'b': 2}
assert tomllib.loads('a=1') == {'a': 1}

# dotted keys
assert tomllib.loads(
    'name = "Orange"\nphysical.color = "orange"\nphysical.shape = "round"\nsite."google.com" = true'
) == {
    'name': 'Orange',
    'physical': {'color': 'orange', 'shape': 'round'},
    'site': {'google.com': True},
}
assert tomllib.loads('fruit.apple.smooth = true\nfruit.orange = 2') == {
    'fruit': {'apple': {'smooth': True}, 'orange': 2}
}
assert tomllib.loads('a . b . c = 1') == {'a': {'b': {'c': 1}}}
assert tomllib.loads('3.14159 = "pi"') == {'3': {'14159': 'pi'}}
assert tomllib.loads('"a.b" . c = 1') == {'a.b': {'c': 1}}

# === tables ===
assert tomllib.loads('[table]') == {'table': {}}
assert tomllib.loads(
    '[table-1]\nkey1 = "some string"\nkey2 = 123\n\n[table-2]\nkey1 = "another string"\nkey2 = 456'
) == {
    'table-1': {'key1': 'some string', 'key2': 123},
    'table-2': {'key1': 'another string', 'key2': 456},
}
assert tomllib.loads('[dog."tater.man"]\ntype.name = "pug"') == {'dog': {'tater.man': {'type': {'name': 'pug'}}}}
assert tomllib.loads('[a.b.c]\n[ d.e.f ]\n[ g .  h  . i ]\n[ j . "ʞ" . \'l\' ]') == {
    'a': {'b': {'c': {}}},
    'd': {'e': {'f': {}}},
    'g': {'h': {'i': {}}},
    'j': {'ʞ': {'l': {}}},
}
assert tomllib.loads('[x.y.z.w]\n\n[x]') == {'x': {'y': {'z': {'w': {}}}}}
assert tomllib.loads('[x]\n[x.y]\n[x.y.z]') == {'x': {'y': {'z': {}}}}
assert tomllib.loads(
    '[fruit]\napple.color = "red"\napple.taste.sweet = true\n\n[fruit.apple.texture]\nsmooth = true'
) == {'fruit': {'apple': {'color': 'red', 'taste': {'sweet': True}, 'texture': {'smooth': True}}}}
assert tomllib.loads('[a]\nb = 1\n[c]\nd = 2\n[a.e]\nf = 3') == {'a': {'b': 1, 'e': {'f': 3}}, 'c': {'d': 2}}
assert tomllib.loads('[a]  # trailing comment\nb = 1  # another') == {'a': {'b': 1}}

# === inline tables ===
assert tomllib.loads('name = { first = "Tom", last = "Preston-Werner" }') == {
    'name': {'first': 'Tom', 'last': 'Preston-Werner'}
}
assert tomllib.loads('point = { x = 1, y = 2 }') == {'point': {'x': 1, 'y': 2}}
assert tomllib.loads('animal = { type.name = "pug" }') == {'animal': {'type': {'name': 'pug'}}}
assert tomllib.loads('empty = {}') == {'empty': {}}
assert tomllib.loads('nested = { a = { b = { c = 1 } } }') == {'nested': {'a': {'b': {'c': 1}}}}
assert tomllib.loads('a = {b.c = 1, b.d = 2}') == {'a': {'b': {'c': 1, 'd': 2}}}
assert tomllib.loads('a = {b = [1, 2], c = "x"}') == {'a': {'b': [1, 2], 'c': 'x'}}

# === arrays ===
assert tomllib.loads('integers = [ 1, 2, 3 ]')['integers'] == [1, 2, 3]
assert tomllib.loads('colors = [ "red", "yellow", "green" ]')['colors'] == ['red', 'yellow', 'green']
assert tomllib.loads('nested = [ [ 1, 2 ], [3, 4, 5] ]')['nested'] == [[1, 2], [3, 4, 5]]
assert tomllib.loads('mixed = [ [ 1, 2 ], ["a", "b", "c"] ]')['mixed'] == [[1, 2], ['a', 'b', 'c']]
assert tomllib.loads('strings = [ "all", \'strings\', """are the same""", \'\'\'type\'\'\' ]')['strings'] == [
    'all',
    'strings',
    'are the same',
    'type',
]
assert tomllib.loads('numbers = [ 0.1, 0.2, 0.5, 1, 2, 5 ]')['numbers'] == [0.1, 0.2, 0.5, 1, 2, 5]
assert tomllib.loads(
    'contributors = [\n  "Foo Bar <foo@example.com>",\n  { name = "Baz Qux", email = "bazqux@example.com", url = "https://example.com/bazqux" }\n]'
)['contributors'] == [
    'Foo Bar <foo@example.com>',
    {'name': 'Baz Qux', 'email': 'bazqux@example.com', 'url': 'https://example.com/bazqux'},
]
assert tomllib.loads('integers2 = [\n  1, 2, 3\n]')['integers2'] == [1, 2, 3]
assert tomllib.loads('integers3 = [\n  1,\n  2, # this is ok\n]')['integers3'] == [1, 2]
assert tomllib.loads('empty = []')['empty'] == []
assert tomllib.loads('empty = [ # comment\n]')['empty'] == []
assert tomllib.loads('x = [1,]')['x'] == [1]
assert tomllib.loads('x = [[]]')['x'] == [[]]
assert tomllib.loads('x = [{}]')['x'] == [{}]
assert tomllib.loads('x = [\n# leading comment\n1\n# trailing comment\n]')['x'] == [1]

# === arrays of tables ===
doc = tomllib.loads("""
[[products]]
name = "Hammer"
sku = 738594937

[[products]]  # empty table within the array

[[products]]
name = "Nail"
sku = 284758393

color = "gray"
""")
assert doc == {
    'products': [
        {'name': 'Hammer', 'sku': 738594937},
        {},
        {'name': 'Nail', 'sku': 284758393, 'color': 'gray'},
    ]
}
doc = tomllib.loads("""
[[fruits]]
name = "apple"

[fruits.physical]  # subtable
color = "red"
shape = "round"

[[fruits.varieties]]  # nested array of tables
name = "red delicious"

[[fruits.varieties]]
name = "granny smith"


[[fruits]]
name = "banana"

[[fruits.varieties]]
name = "plantain"
""")
assert doc == {
    'fruits': [
        {
            'name': 'apple',
            'physical': {'color': 'red', 'shape': 'round'},
            'varieties': [{'name': 'red delicious'}, {'name': 'granny smith'}],
        },
        {'name': 'banana', 'varieties': [{'name': 'plantain'}]},
    ]
}
assert tomllib.loads('[[a]]\n[a.b]\n[[a]]\n[a.b]') == {'a': [{'b': {}}, {'b': {}}]}
assert tomllib.loads('[[a.b]]\nc = 1\n[[a.b]]\nc = 2') == {'a': {'b': [{'c': 1}, {'c': 2}]}}
assert tomllib.loads('[[ spaced ]]\nx = 1') == {'spaced': [{'x': 1}]}
assert tomllib.loads('points = [ { x = 1, y = 2, z = 3 },\n           { x = 7, y = 8, z = 9 } ]') == {
    'points': [{'x': 1, 'y': 2, 'z': 3}, {'x': 7, 'y': 8, 'z': 9}]
}

# === dates and times ===
doc = tomllib.loads("""
odt1 = 1979-05-27T07:32:00Z
odt2 = 1979-05-27T00:32:00-07:00
odt3 = 1979-05-27T00:32:00.999999-07:00
odt4 = 1979-05-27 07:32:00Z
odt5 = 1979-05-27t07:32:00z
odt6 = 1979-05-27T07:32:00+05:30
ldt1 = 1979-05-27T07:32:00
ldt2 = 1979-05-27T00:32:00.999999
ldt3 = 1979-05-27T00:32:00.5
ldt4 = 1979-05-27T00:32:00.1234567
ld1 = 1979-05-27
lt1 = 07:32:00
lt2 = 00:32:00.999999
lt3 = 00:32:00.1
lt4 = 23:59:59
""")
assert doc['odt1'] == datetime(1979, 5, 27, 7, 32, tzinfo=timezone.utc)
assert doc['odt1'].tzinfo is timezone.utc
assert doc['odt2'] == datetime(1979, 5, 27, 0, 32, tzinfo=timezone(timedelta(hours=-7)))
assert doc['odt2'].utcoffset() == timedelta(hours=-7)
assert doc['odt3'] == datetime(1979, 5, 27, 0, 32, 0, 999999, tzinfo=timezone(timedelta(hours=-7)))
assert doc['odt4'] == datetime(1979, 5, 27, 7, 32, tzinfo=timezone.utc)
assert doc['odt5'] == datetime(1979, 5, 27, 7, 32, tzinfo=timezone.utc)
assert doc['odt6'].utcoffset() == timedelta(hours=5, minutes=30)
assert doc['ldt1'] == datetime(1979, 5, 27, 7, 32)
assert doc['ldt1'].tzinfo is None
assert doc['ldt2'] == datetime(1979, 5, 27, 0, 32, 0, 999999)
assert doc['ldt3'] == datetime(1979, 5, 27, 0, 32, 0, 500000)
assert doc['ldt4'] == datetime(1979, 5, 27, 0, 32, 0, 123456)
assert doc['ld1'] == date(1979, 5, 27)
assert type(doc['ld1']) is date
assert doc['lt1'] == time(7, 32)
assert doc['lt2'] == time(0, 32, 0, 999999)
assert doc['lt3'] == time(0, 32, 0, 100000)
assert doc['lt4'] == time(23, 59, 59)
assert (
    repr(doc['odt2'])
    == 'datetime.datetime(1979, 5, 27, 0, 32, tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=61200)))'
)
assert tomllib.loads('x = 0001-01-01')['x'] == date(1, 1, 1)
assert tomllib.loads('x = 9999-12-31T23:59:59.999999')['x'] == datetime(9999, 12, 31, 23, 59, 59, 999999)
assert tomllib.loads('x = 2024-02-29')['x'] == date(2024, 2, 29)
assert tomllib.loads('x = [1979-05-27, 07:32:00]')['x'] == [date(1979, 5, 27), time(7, 32)]

# === comments ===
assert tomllib.loads(
    '# This is a full-line comment\nkey = "value"  # This is a comment at the end of a line\nanother = "# This is not a comment"'
) == {
    'key': 'value',
    'another': '# This is not a comment',
}
assert tomllib.loads('x = 1 # comment with unicode ☃ and tab\t') == {'x': 1}
assert tomllib.loads('#') == {}
assert tomllib.loads('# no newline at end') == {}

# === duplicate-free but repeated structure ===
assert tomllib.loads('[a]\nb = 1\n\n[a.c]\nd = 2') == {'a': {'b': 1, 'c': {'d': 2}}}
assert tomllib.loads('a.b.c = 1\na.b.d = 2\n[a.e]\nf = 3') == {'a': {'b': {'c': 1, 'd': 2}, 'e': {'f': 3}}}

# === the returned containers are ordinary mutable objects ===
doc = tomllib.loads('[a]\nb = [1]')
doc['a']['b'].append(2)
doc['a']['c'] = 3
assert doc == {'a': {'b': [1, 2], 'c': 3}}
assert type(doc) is dict
assert type(doc['a']['b']) is list

# === a realistic pyproject.toml ===
pyproject = tomllib.loads("""
[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[project]
name = "example"
version = "0.1.0"
description = "An example project"
readme = "README.md"
requires-python = ">=3.9"
license = { text = "MIT" }
authors = [
    { name = "Samuel Colvin", email = "s@muelcolvin.com" },
]
classifiers = [
    "Programming Language :: Python :: 3",
    "License :: OSI Approved :: MIT License",
]
dependencies = [
    "pydantic>=2",
    "httpx",
]

[project.optional-dependencies]
dev = ["pytest", "ruff"]

[project.urls]
Homepage = "https://example.com"

[tool.ruff]
line-length = 120
target-version = "py39"

[tool.ruff.lint]
extend-select = ["Q", "RUF100", "UP", "I"]
isort = { combine-as-imports = true, known-first-party = ["example"] }

[tool.pytest.ini_options]
testpaths = ["tests"]
filterwarnings = [
    "error",
]
""")
assert pyproject['build-system'] == {'requires': ['hatchling'], 'build-backend': 'hatchling.build'}
assert pyproject['project']['name'] == 'example'
assert pyproject['project']['authors'] == [{'name': 'Samuel Colvin', 'email': 's@muelcolvin.com'}]
assert pyproject['project']['dependencies'] == ['pydantic>=2', 'httpx']
assert pyproject['project']['optional-dependencies'] == {'dev': ['pytest', 'ruff']}
assert pyproject['tool']['ruff'] == {
    'line-length': 120,
    'target-version': 'py39',
    'lint': {
        'extend-select': ['Q', 'RUF100', 'UP', 'I'],
        'isort': {'combine-as-imports': True, 'known-first-party': ['example']},
    },
}
assert list(pyproject) == ['build-system', 'project', 'tool']
assert list(pyproject['tool']) == ['ruff', 'pytest']
