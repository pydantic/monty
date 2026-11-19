# === Type-less spec renders like str(), padded as one string ===
assert format(1 + 2j, '') == '(1+2j)'
assert format(1j, '') == '1j'
assert format(complex(-0.0, 1), '') == '(-0+1j)'
assert format(1 - 2.5j, '') == '(1-2.5j)'
assert format(complex(0.0, -1), '') == '-1j'
assert format(-1 - 1j, '') == '(-1-1j)'
assert format(complex(float('nan'), float('inf')), '') == '(nan+infj)'
assert format(complex(0, float('nan')), '') == 'nanj'
assert format(complex(1e20, 1), '') == '(1e+20+1j)'
assert format(complex(1e15, 1), '') == '(1000000000000000+1j)'
assert format(complex(1e16, 1), '') == '(1e+16+1j)'
assert format(complex(1e-5, 1), '') == '(1e-05+1j)'
assert format(1 + 2j, '>12') == '      (1+2j)'
assert format(1j, '>12') == '          1j'
assert format(1 + 2j, '<12') == '(1+2j)      '
assert format(1 + 2j, '^12') == '   (1+2j)   '
assert format(complex(-0.0, 1), '^12') == '  (-0+1j)   '
assert format(1 + 2j, '*^12') == '***(1+2j)***'
assert format(1j, 'x>5') == 'xxx1j'
assert format(1j, '5') == '   1j'
assert format(1j, '<5') == '1j   '
assert f'{1 + 2j:>10}' == '    (1+2j)'
assert '{:.2f}'.format(1j) == '0.00+1.00j'
assert f'{1j:{""}}' == '1j'

# === Sign applies to the real part, or the lone imaginary part ===
assert format(1 + 2j, '+') == '(+1+2j)'
assert format(1j, '+') == '+1j'
assert format(complex(-0.0, 1), '+') == '(-0+1j)'
assert format(1 - 2.5j, '+') == '(+1-2.5j)'
assert format(complex(0.0, -1), '+') == '-1j'
assert format(1 + 2j, ' ') == '( 1+2j)'
assert format(1j, ' ') == ' 1j'
assert format(1 + 2j, '-') == '(1+2j)'
assert format(1 + 2j, '+.1f') == '+1.0+2.0j'
assert format(1j, '+.1f') == '+0.0+1.0j'
assert format(complex(0.0, -1), '+.1f') == '+0.0-1.0j'
assert format(1 + 2j, '>+12.1f') == '   +1.0+2.0j'
assert format(1 + 2j, ' 12.1f') == '    1.0+2.0j'

# === Precision without a type is g; a type drops the parentheses ===
assert format(1 + 2j, '.2') == '(1+2j)'
assert format(1j, '.2') == '1j'
assert format(complex(1.0, 2.0), '.0') == '(1+2j)'
assert format(complex(1.5, 1), '.0') == '(2+1j)'
assert format(complex(0.5, 1), '.0') == '(0.5+1j)'
assert format(complex(12345.678, 2.0), '.3') == '(1.23e+04+2j)'
assert format(complex(123456.789, 1), '.2') == '(1.2e+05+1j)'
assert format(1 + 2j, '.2f') == '1.00+2.00j'
assert format(1j, '.2f') == '0.00+1.00j'
assert format(complex(-0.0, 1), '.2f') == '-0.00+1.00j'
assert format(1 - 2.5j, '.2f') == '1.00-2.50j'
assert format(complex(0.0, -1), '.2f') == '0.00-1.00j'
assert format(complex(1.0, 2.0), '.0f') == '1+2j'
assert format(complex(1.5, 1), '.0f') == '2+1j'
assert format(1 + 2j, '12.2f') == '  1.00+2.00j'
assert format(complex(-0.0, 1), '12.2f') == ' -0.00+1.00j'
assert format(1 + 2j, '.2e') == '1.00e+00+2.00e+00j'
assert format(complex(-0.0, 1), '.2e') == '-0.00e+00+1.00e+00j'
assert format(1 + 2j, 'g') == '1+2j'
assert format(1j, 'g') == '0+1j'
assert format(complex(-0.0, 1), 'g') == '-0+1j'
assert format(1 - 2.5j, '.3g') == '1-2.5j'
assert format(1 + 2j, 'n') == '1+2j'
assert format(1j, '.3n') == '0+1j'
assert format(complex(1e20, 1), 'g') == '1e+20+1j'
assert format(complex(1e-5, 1), 'g') == '1e-05+1j'
assert format(1j, 'F') == '0.000000+1.000000j'
assert format(1j, '#g') == '0.00000+1.00000j'
assert format(complex(float('nan'), float('inf')), '.2f') == 'nan+infj'
assert format(complex(float('nan'), float('inf')), 'F') == 'NAN+INFj'
assert format(complex(float('nan'), float('inf')), 'E') == 'NAN+INFj'
assert format(complex(float('nan'), float('inf')), 'G') == 'NAN+INFj'

# === Alternate form, grouping and negative-zero coercion ===
assert format(1 + 2j, '#') == '(1.+2.j)'
assert format(1j, '#') == '1.j'
assert format(complex(-0.0, 1), '#') == '(-0.+1.j)'
assert format(1 - 2.5j, '#') == '(1.-2.5j)'
assert format(complex(1e20, 1), '#') == '(1.e+20+1.j)'
assert format(1 + 2j, '#.0f') == '1.+2.j'
assert format(1 - 2.5j, '#.0f') == '1.-2.j'
assert format(complex(1000000, 2000000), ',') == '(1,000,000+2,000,000j)'
assert format(complex(1000000, 2), ',.1f') == '1,000,000.0+2.0j'
assert format(complex(1234567.0, 2), '_') == '(1_234_567+2j)'
assert format(complex(1234567.0, 2), ',g') == '1.23457e+06+2j'
assert format(complex(1234567.0, 2), ',.10g') == '1,234,567+2j'
assert format(complex(1000000, 2000000), '_g') == '1e+06+2e+06j'
assert format(1 + 2j, '.2_f') == '1.00+2.00j'
assert format(1j, 'z') == '1j'
assert format(1j, 'zf') == '0.000000+1.000000j'
assert format(complex(-0.0, 1), 'z.2f') == '0.00+1.00j'
assert format(complex(-0.0, -0.0), 'z') == '(0+0j)'
assert format(complex(-0.0, -0.0), 'zf') == '0.000000+0.000000j'
assert format(complex(-0.0, -0.0), 'z.0f') == '0+0j'
assert format(complex(-0.0, -0.0), 'zg') == '0+0j'
assert format(complex(-0.0, -0.0), 'z.1') == '(0+0j)'
assert format(complex(-0.0, -0.0), '') == '(-0-0j)'

# === Rejected specs ===
for spec, message in [
    ('%', "Unknown format code '%' for object of type 'complex'"),
    ('.2%', "Unknown format code '%' for object of type 'complex'"),
    ('d', "Unknown format code 'd' for object of type 'complex'"),
    ('s', "Unknown format code 's' for object of type 'complex'"),
    ('.2s', "Unknown format code 's' for object of type 'complex'"),
    ('x', "Unknown format code 'x' for object of type 'complex'"),
    ('b', "Unknown format code 'b' for object of type 'complex'"),
    ('c', "Unknown format code 'c' for object of type 'complex'"),
    ('#c', "Unknown format code 'c' for object of type 'complex'"),
    ('#s', "Unknown format code 's' for object of type 'complex'"),
    ('zd', "Unknown format code 'd' for object of type 'complex'"),
    ('0d', "Unknown format code 'd' for object of type 'complex'"),
    (',d', "Unknown format code 'd' for object of type 'complex'"),
    ('=d', "Unknown format code 'd' for object of type 'complex'"),
    ('010', 'Zero padding is not allowed in complex format specifier'),
    ('05', 'Zero padding is not allowed in complex format specifier'),
    ('0', 'Zero padding is not allowed in complex format specifier'),
    ('0>5', 'Zero padding is not allowed in complex format specifier'),
    ('0=10', 'Zero padding is not allowed in complex format specifier'),
    ('0,', 'Zero padding is not allowed in complex format specifier'),
    ('=10', "'=' alignment flag is not allowed in complex format specifier"),
    ('=', "'=' alignment flag is not allowed in complex format specifier"),
    ('=,', "'=' alignment flag is not allowed in complex format specifier"),
    ('#=', "'=' alignment flag is not allowed in complex format specifier"),
    ('=#', "'=' alignment flag is not allowed in complex format specifier"),
    ('=.2', "'=' alignment flag is not allowed in complex format specifier"),
    (',c', "Cannot specify ',' with 'c'."),
    (',s', "Cannot specify ',' with 's'."),
    ('z_c', "Cannot specify '_' with 'c'."),
    ('.-1f', 'Format specifier missing precision'),
]:
    try:
        format(1j, spec)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == message
