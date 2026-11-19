from decimal import ROUND_DOWN, Clamped, Context, Decimal, Inexact, Overflow, Rounded, localcontext


operations = (
    lambda ctx, a, b: ctx.divide_int(a, b),
    lambda ctx, a, b: a // b,
    lambda ctx, a, b: ctx.divmod(a, b)[0],
    lambda ctx, a, b: divmod(a, b)[0],
)

for divide in operations:
    # An integer quotient can exceed Emax when the precision is wider than its exponent range.
    for sign in ('', '-'):
        with localcontext(Context(prec=28, Emax=10, Emin=-10, traps=[])) as ctx:
            result = divide(ctx, Decimal(sign + '1e20'), Decimal(1))
            assert str(result) == sign + 'Infinity'
            assert ctx.flags[Overflow]
            assert ctx.flags[Inexact]
            assert ctx.flags[Rounded]

        with localcontext(Context(prec=28, Emax=10, Emin=-10, rounding=ROUND_DOWN, traps=[])) as ctx:
            result = divide(ctx, Decimal(sign + '1e20'), Decimal(1))
            assert result == Decimal(sign + '9' * 28 + 'e-17')
            assert ctx.flags[Overflow]
            assert ctx.flags[Inexact]
            assert ctx.flags[Rounded]

    with localcontext(Context(prec=28, Emax=10, Emin=-10, traps=[Overflow])) as ctx:
        try:
            divide(ctx, Decimal('1e20'), Decimal(1))
            assert False, 'expected Overflow'
        except Overflow:
            pass

        assert ctx.flags[Overflow]
        assert ctx.flags[Inexact]
        assert ctx.flags[Rounded]

    with localcontext(Context(prec=9, Emax=5, Emin=-30, clamp=1, traps=[])) as ctx:
        result = divide(ctx, Decimal('3e3'), Decimal(-10))
        assert str(result) == '-300.000'
        assert ctx.flags[Clamped]
        assert not ctx.flags[Inexact]
        assert not ctx.flags[Rounded]

    with localcontext(Context(prec=9, Emax=5, Emin=-30, clamp=1, traps=[Clamped])) as ctx:
        try:
            divide(ctx, Decimal('3e3'), Decimal(-10))
            assert False, 'expected Clamped'
        except Clamped:
            pass

        assert ctx.flags[Clamped]

# Finalizing the quotient must preserve the exact remainder.
with localcontext(Context(prec=28, Emax=10, Emin=-10, traps=[])) as ctx:
    assert ctx.divmod(Decimal('100000000000000000001'), Decimal(1)) == (Decimal('Infinity'), Decimal(0))
    assert divmod(Decimal('100000000000000000001'), Decimal(2)) == (Decimal('Infinity'), Decimal(1))

with localcontext(Context(prec=9, Emax=5, Emin=-30, clamp=1, traps=[])) as ctx:
    assert [str(x) for x in ctx.divmod(Decimal('3001'), Decimal(-10))] == ['-300.000', '1.000']
    assert [str(x) for x in divmod(Decimal('3001'), Decimal(-10))] == ['-300.000', '1.000']
