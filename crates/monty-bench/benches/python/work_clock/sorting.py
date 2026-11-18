values = [(i * 7919) % 100_003 for i in range(100_000)]
ordered = sorted(values)
assert ordered[0] == 0
assert ordered[-1] == 100002
words = [str(v) for v in values[:30_000]]
words.sort()
assert words[0] == '0'
pairs = sorted(zip(values[:30_000], words), reverse=True)
assert pairs[0][0] == max(values[:30_000])
by_key = sorted(values[:30_000], key=lambda v: v % 1000)
assert by_key[0] % 1000 == 0
