from decimal import Decimal as D

# === Empty spec falls back to str() (may be scientific) ===
assert f'{D("1.20")}' == '1.20', 'empty spec keeps trailing zero'
assert f'{D("1E+5")}' == '1E+5', 'empty spec keeps exponent'
assert f'{D("1.20E+5")}' == '1.20E+5', 'empty spec scaled exponent'

# === Fixed (f) — native digits, never via f64 ===
assert f'{D("1.5"):.2f}' == '1.50', 'fixed pads fraction'
assert f'{D("1.10"):.2f}' == '1.10', 'fixed keeps value'
assert f'{D("1.10"):.20f}' == '1.10000000000000000000', 'fixed 20 places stays exact (not f64)'
assert f'{D("2.5"):.0f}' == '2', 'fixed 0 places half-even'
assert f'{D("2.675"):.2f}' == '2.68', 'fixed rounds half-even'
assert f'{D("1.5"):f}' == '1.5', 'fixed default precision uses value'

# === Fill / align / width / zero-pad ===
assert f'{D("1.5"):10.2f}' == '      1.50', 'width right-align default'
assert f'{D("1.5"):<10.2f}' == '1.50      ', 'left align'
assert f'{D("1.5"):>10.2f}' == '      1.50', 'right align'
assert f'{D("1.5"):^10.2f}' == '   1.50   ', 'center align'
assert f'{D("1.5"):010.2f}' == '0000001.50', 'zero pad'
assert f'{D("1.5"):*^10}' == '***1.5****', 'custom fill center'

# === Sign ===
assert f'{D("1.5"):+.2f}' == '+1.50', 'plus sign'
assert f'{D("1.5"): .2f}' == ' 1.50', 'space sign'
assert f'{D("-1.5"):+.2f}' == '-1.50', 'negative keeps minus'

# === Grouping ===
assert f'{D("1234567")}:{D("1234567"):,}' == '1234567:1,234,567', 'comma grouping'
assert f'{D("1234567.891"):,.2f}' == '1,234,567.89', 'money format'
assert f'{D("1234567"):_}' == '1_234_567', 'underscore grouping'

# === Scientific (e / E) — no two-digit exponent padding ===
assert f'{D("1234.5"):.3e}' == '1.234e+3', 'scientific lowercase'
assert f'{D("1234.5"):.3E}' == '1.234E+3', 'scientific uppercase'
assert f'{D("1.5"):e}' == '1.5e+0', 'scientific default'

# === General (g) ===
assert f'{D("1234.5"):.3g}' == '1.23e+3', 'general picks scientific'
assert f'{D("0.0001234"):.3g}' == '0.000123', 'general picks fixed'
assert f'{D("1.5"):g}' == '1.5', 'general default'

# === Percent ===
assert f'{D("0.1234"):.2%}' == '12.34%', 'percent'
assert f'{D("0.5"):%}' == '50%', 'percent default'

# === Precision without a type behaves like g ===
assert f'{D("1.23456"):.4}' == '1.235', 'precision-only short'
assert f'{D("123.456"):.2}' == '1.2E+2', 'precision-only scientific'

# === Specials ===
assert f'{D("inf"):f}' == 'Infinity', 'infinity'
assert f'{D("nan")}' == 'NaN', 'nan'
assert f'{D("-2.5"):.2f}' == '-2.50', 'negative fixed'

# === General (g) keeps significant trailing zeros (unlike float g) ===
assert f'{D("1.20"):g}' == '1.20', 'g keeps trailing zero'
assert f'{D("120.0"):g}' == '120.0', 'g keeps trailing zero with int part'
assert f'{D("1.0"):.3g}' == '1.0', 'g precision does not pad short value'
assert f'{D("1.2300"):.6g}' == '1.2300', 'g keeps zeros within precision'
assert f'{D("1000"):.3g}' == '1.00e+3', 'g scientific keeps trailing zeros'
assert f'{D("1200"):.3g}' == '1.20e+3', 'g scientific keeps one trailing zero'
assert f'{D("100"):.2g}' == '1.0e+2', 'g scientific pads to precision'
assert f'{D("1.23456789"):g}' == '1.23456789', 'g with no precision keeps all digits'

# === General (g) zero keeps its exponent ===
assert f'{D("0.00"):.3g}' == '0.00', 'g zero keeps scale'
assert f'{D("0E-5"):g}' == '0.00000', 'g zero keeps negative exponent'
assert f'{D("0E+1"):g}' == '0e+1', 'g zero with positive exponent stays scientific'

# === General (g) fixed/scientific threshold matches decimal (not float) ===
assert f'{D("1.23e-5"):g}' == '0.0000123', 'g fixed down to leftdigits > -6'
assert f'{D("1.23e-6"):g}' == '0.00000123', 'g still fixed at the -6 boundary'

# === Precision without a type uses uppercase G, keeps zeros ===
assert f'{D("1.20"):.3}' == '1.20', 'precision-only keeps trailing zero'
assert f'{D("0.00"):.3}' == '0.00', 'precision-only zero keeps scale'
assert f'{D("0E+1"):.3}' == '0E+1', 'precision-only zero positive exponent'
assert f'{D("1000"):.3}' == '1.00E+3', 'precision-only uppercase scientific'

# === Scientific (e) zero shifts the exponent by the precision ===
assert f'{D("0"):.2e}' == '0.00e+2', 'e zero exponent shifted by precision'
assert f'{D("0"):.3e}' == '0.000e+3', 'e zero exponent shifted by precision (3)'
assert f'{D("0E+5"):.2e}' == '0.00e+7', 'e zero adds value exponent and precision'
assert f'{D("0"):e}' == '0e+0', 'e zero no precision'
assert f'{D("0.00"):e}' == '0e-2', 'e zero no precision keeps exponent'

# === Rounding carry recomputes the digit count (no stray trailing digit) ===
assert f'{D("9.99"):.1e}' == '1.0e+1', 'e carry collapses to one fractional digit'
assert f'{D("9.6"):.0e}' == '1e+1', 'e carry with zero precision has no fraction'
assert f'{D("9.99"):.2}' == '10', 'precision-only carry drops trailing zero'
assert f'{D("99.5"):.2g}' == '1.0e+2', 'g half-even carry to power of ten'

# === Percent: suffix on non-finite, zero with positive exponent ===
assert f'{D("inf"):%}' == 'Infinity%', 'percent on infinity keeps suffix'
assert f'{D("-inf"):.2%}' == '-Infinity%', 'percent on -infinity keeps sign and suffix'
assert f'{D("nan"):.2%}' == 'NaN%', 'percent on nan keeps suffix'
assert f'{D("0"):%}' == '0%', 'percent of zero is 0%, not 000%'
assert f'{D("-0"):%}' == '-0%', 'percent of negative zero'

# === Fixed: zero with positive exponent renders as a single 0 ===
assert f'{D("0E2"):f}' == '0', 'fixed zero with positive exponent'
assert f'{D("0E+3"):f}' == '0', 'fixed zero with positive exponent (3)'
assert f'{D("0.00"):f}' == '0.00', 'fixed zero with scale keeps zeros'

# === Alternate form (#) keeps a trailing decimal point ===
assert f'{D("5"):#g}' == '5.', 'alternate g keeps point'
assert f'{D("5"):#e}' == '5.e+0', 'alternate e keeps point'
assert f'{D("5"):#.0f}' == '5.', 'alternate fixed keeps point'

# === Unsupported specs raise CPython's single 'invalid format string' ===
for bad in ['d', 'x', 'X', 'b', 'o', 'c', 's', ',n', '_n', ',d', 'zd', 'zs']:
    try:
        _ = f'{D("123"):{bad}}'
        assert False, f'expected {bad!r} to raise'
    except ValueError as exc:
        assert str(exc) == 'invalid format string', f'{bad!r}: {exc}'

# Invalid specs are rejected even for non-finite values (spec parsed first).
for bad in ['d', ',n']:
    try:
        _ = f'{D("inf"):{bad}}'
        assert False, f'expected {bad!r} on inf to raise'
    except ValueError as exc:
        assert str(exc) == 'invalid format string', f'inf {bad!r}: {exc}'

# === z (negative-zero coercion) is honored for Decimal ===
assert f'{D("-0.001"):z.2f}' == '0.00', 'z coerces rounded negative zero'
assert f'{D("-0.001"):.2f}' == '-0.00', 'without z the minus survives'

# === Specials ignore the `0` flag (space-filled) but honor explicit fills ===
assert f'{D("NaN"):010}' == '       NaN', 'NaN 0-flag space-fills'
assert f'{D("NaN"):0>10}' == '0000000NaN', 'explicit 0 fill honored'
assert f'{D("NaN"):0=10}' == '0000000NaN', 'explicit 0= fill honored'
assert f'{D("-Infinity"):015f}' == '      -Infinity', '-Infinity 0-flag space-fills'
assert f'{D("NaN"):*>10}' == '*******NaN', 'explicit * fill honored'
assert f'{D("inf"):+010.3e}' == ' +Infinity', 'inf 0-flag with sign space-fills'
assert f'{D("Infinity"):010}' == '  Infinity', 'Infinity 0-flag space-fills'
