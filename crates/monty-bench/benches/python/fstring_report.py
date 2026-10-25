# Report formatting with f-strings: the format mini-language, float formatting and
# `str.join` over a large list of tracked strings.

total = 0.0
lines = []
for i in range(2_000):
    name = 'product-' + str(i % 100)
    price = (i * 7) % 300 + 0.99
    share = (i % 100) / 100
    total += price
    lines.append(f'| {name:<14} | {price:>10.2f} | {share:>7.1%} | {i:06d} |')
lines.append(f'total: {total:,.2f}')
report = '\n'.join(lines)
assert len(report) == 102_017
