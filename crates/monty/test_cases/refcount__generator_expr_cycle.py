def make_cycle():
    holder = []
    generator = (holder for _ in [0])
    holder.append(generator)
    return generator


cycle = make_cycle()
next(cycle)
cycle = None

# Trigger the periodic collector after the cycle loses its external root.
for _ in range(1100):
    item = []

# ref-counts={'item': 1}
