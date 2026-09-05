def capture_error(template, value):
    try:
        template.format(value)
    except Exception as exc:
        return type(exc).__name__, str(exc)
    return None


long_int = 10**20
assert '{:b}'.format(long_int) == ('1010110101111000111010111100010110101100011000100000000000000000000')
assert '{:f}'.format(long_int) == '100000000000000000000.000000'
assert '{:F}'.format(long_int) == '100000000000000000000.000000'
assert '{:e}'.format(long_int) == '1.000000e+20'
assert '{:E}'.format(long_int) == '1.000000E+20'
assert '{:g}'.format(long_int) == '1e+20'
assert '{:G}'.format(long_int) == '1E+20'
assert '{:%}'.format(long_int) == '10000000000000000000000.000000%'
assert capture_error('{:f}', 10**1000) == (
    'OverflowError',
    'int too large to convert to float',
)
assert capture_error('{:c}', long_int) == (
    'OverflowError',
    'Python int too large to convert to C long',
)
assert capture_error('{:s}', long_int) == (
    'ValueError',
    "Unknown format code 's' for object of type 'int'",
)
