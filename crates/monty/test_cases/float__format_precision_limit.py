# CPython 3.14.8 added this bound in PyOS_double_to_string, before finite-value handling.
# https://github.com/python/cpython/blob/v3.14.8/Python/pystrtod.c
limit = 2**31 - 1 - 1024

for factory in [str, bytes, bytearray]:
    for code in ['g', 'G', 'f', 'F', 'e', 'E']:
        text = '%.*' + code
        template = text if factory is str else factory(text, 'ascii')
        expected = 'INF' if code in ['G', 'F', 'E'] else 'inf'
        expected = expected if factory is str else factory(expected, 'ascii')
        assert template % (limit, float('inf')) == expected
        for value in [1.0, float('inf'), float('-inf'), float('nan')]:
            try:
                template % (limit + 1, value)
            except ValueError as exc:
                assert str(exc) == 'precision too big'
            else:
                assert False, (factory, code, value)

    text = '%.*g'
    template = text if factory is str else factory(text, 'ascii')
    expected = '1' if factory is str else factory('1', 'ascii')
    assert template % (limit, 1.0) == expected

for code in ['g', 'G', 'f', 'F', 'e', 'E', '%', 'n', '']:
    spec = f'.{limit + 1}{code}'
    for value in [1, 1.0, float('inf'), float('nan')]:
        try:
            format(value, spec)
        except ValueError as exc:
            expected = (
                'Precision not allowed in integer format specifier'
                if type(value) is int and code in ['n', '']
                else 'precision too big'
            )
            assert str(exc) == expected, (code, value, str(exc))
        else:
            assert False, (code, value)

assert format(1.0, f'.{limit}g') == '1'
