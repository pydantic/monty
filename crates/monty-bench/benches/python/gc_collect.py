# Single-collection latency: 50,000 self-referencing lists, cycles that reference counting
# alone cannot free, collected at once.

import gc

gc.disable()
for _ in range(50_000):
    a = []
    a.append(a)
gc.collect()
