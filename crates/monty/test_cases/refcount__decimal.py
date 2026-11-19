# Signal dicts and `localcontext()` managers own references to their contexts;
# dropping them must release those references.
from decimal import Context, Decimal, Inexact, getcontext, localcontext

c = Context(prec=5)
flags = c.flags
flags[Inexact] = True
traps = c.traps
flags = traps = None
for _ in range(3):
    getcontext().flags[Inexact] = False
    c.traps.copy()
with localcontext() as local:
    d = Decimal(1) / 3
with localcontext(c):
    pass
manager = localcontext(prec=3)
manager = None

c
# ref-counts={'c': 2, 'd': 1, 'local': 1}
