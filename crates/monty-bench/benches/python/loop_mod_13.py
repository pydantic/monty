# /// monty
# [bench]
# limits = true
# ///
# A tight loop with a branch and string concatenation.
# `limits` also runs it with resource limits armed, pricing the amortized limit checks.

v = ''
for i in range(1_000):
    if i % 13 == 0:
        v += 'x'
assert len(v) == 77
