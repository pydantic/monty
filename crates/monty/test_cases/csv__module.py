import csv
import sys

is_monty = sys.platform == 'monty'

# === Constants ===
assert (csv.QUOTE_MINIMAL, csv.QUOTE_ALL, csv.QUOTE_NONNUMERIC, csv.QUOTE_NONE) == (0, 1, 2, 3)
assert (csv.QUOTE_STRINGS, csv.QUOTE_NOTNULL) == (4, 5)

# === Error ===
assert str(csv.Error) == "<class '_csv.Error'>"
exc = csv.Error('oops')
assert isinstance(exc, Exception)
assert not isinstance(exc, ValueError)
assert str(exc) == 'oops'
assert exc.args == ('oops',)
try:
    raise csv.Error('raised')
except Exception as caught:
    assert isinstance(caught, csv.Error)
    assert str(caught) == 'raised'

# === Dialects ===
assert csv.list_dialects() == ['excel', 'excel-tab', 'unix']
d = csv.get_dialect('excel')
assert str(type(d)) == "<class '_csv.Dialect'>"
assert repr(d).startswith('<_csv.Dialect object at 0x')
assert (d.delimiter, d.quotechar, d.escapechar) == (',', '"', None)
assert (d.doublequote, d.skipinitialspace, d.strict) == (True, False, False)
assert (d.lineterminator, d.quoting) == ('\r\n', csv.QUOTE_MINIMAL)
assert type(d.quoting) is int
tab = csv.get_dialect('excel-tab')
assert (tab.delimiter, tab.lineterminator) == ('\t', '\r\n')
unix = csv.get_dialect('unix')
assert (unix.delimiter, unix.lineterminator, unix.quoting) == (',', '\n', csv.QUOTE_ALL)
try:
    d.delimiter = ';'
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "attribute 'delimiter' of '_csv.Dialect' objects is not writable"
try:
    d.nope
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'_csv.Dialect' object has no attribute 'nope'"
assert csv.get_dialect(name='unix').quoting == csv.QUOTE_ALL
for args, kwargs, message in (
    ((), {}, "get_dialect() missing required argument 'name' (pos 1)"),
    ((1, 2), {}, 'get_dialect() takes at most 1 argument (2 given)'),
    ((), {'nope': 1}, "get_dialect() missing required argument 'name' (pos 1)"),
    (('excel',), {'name': 'excel'}, 'get_dialect() takes at most 1 argument (2 given)'),
):
    try:
        csv.get_dialect(*args, **kwargs)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message
for name in ('nope', 5):
    try:
        csv.get_dialect(name)
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'unknown dialect'

# The built-in dialects expose their parameters as attributes.
assert (csv.excel.delimiter, csv.excel.lineterminator) == (',', '\r\n')
assert csv.excel_tab.delimiter == '\t'
assert (csv.unix_dialect.quoting, csv.unix_dialect.lineterminator) == (csv.QUOTE_ALL, '\n')

# `csv.Dialect` is `_csv.Dialect` in Monty, which takes the format parameters; CPython's
# is a Python base class whose constructor takes none (see limitations/csv.md).
assert csv.Dialect.__name__ == 'Dialect'
if is_monty:
    made = csv.Dialect(delimiter=';')
    assert (made.delimiter, made.lineterminator, type(made)) == (';', '\r\n', type(d))
else:
    try:
        csv.Dialect(delimiter=';')
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "Dialect.__init__() got an unexpected keyword argument 'delimiter'"

# `_csv.Dialect(...)` resolves parameters the same way the reader does.
Dialect = type(d)
assert Dialect(delimiter=';').delimiter == ';'
assert Dialect('excel-tab', delimiter=';').lineterminator == '\r\n'
assert Dialect(5).delimiter == ','
assert Dialect.__name__ == 'Dialect'
try:
    Dialect(quoting=7)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'bad "quoting" value'

# === field_size_limit ===
assert csv.field_size_limit() == 131072
try:
    csv.field_size_limit('x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'limit must be an integer'
for args, kwargs, message in (
    ((1, 2), {}, 'field_size_limit() takes at most 1 argument (2 given)'),
    ((), {'nope': 1}, "field_size_limit() got an unexpected keyword argument 'nope'"),
):
    try:
        csv.field_size_limit(*args, **kwargs)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message
if is_monty:
    for args, kwargs in (((10,), {}), ((), {'new_limit': 10})):
        try:
            csv.field_size_limit(*args, **kwargs)
            assert False, 'expected NotImplementedError'
        except NotImplementedError as exc:
            assert str(exc) == 'field_size_limit(): changing the limit is not supported'
