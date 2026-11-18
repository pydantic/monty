values = []
for i in range(100_000):
    values.append(i * 7 % 1000)
doubled = values + values
doubled.reverse()
assert sum(doubled) == 99900000
assert max(doubled) == 999
assert doubled.count(3) == 200
assert doubled[10:50_000:3][-1] == doubled[49_999 - (49_989 % 3)]
assert len(list(zip(values, doubled))) == 100_000
