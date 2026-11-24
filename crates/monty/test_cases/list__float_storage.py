import math

values = [0.25, -0.0, float('inf'), float('-inf'), float('nan')]
fraction, signed_zero, positive_inf, negative_inf, nan = values
assert fraction == 0.25 and math.copysign(1.0, signed_zero) == -1.0
assert positive_inf == float('inf') and negative_inf == float('-inf') and math.isnan(nan)
copies = values.copy() + values[::-1]
assert copies[0] == 0.25 and math.copysign(1.0, copies[1]) == -1.0
assert copies[2] == float('inf') and copies[3] == float('-inf')
assert math.isnan(copies[4]) and math.isnan(copies[5])
values.reverse()
assert math.isnan(values.pop(0)) and math.copysign(1.0, values[-2]) == -1.0

values = [1.25, 2.5, 3.75]
values[::-1] = values
values[1:2] = values
assert values == [3.75, 3.75, 2.5, 1.25, 1.25]
values.insert(2, -0.0)
assert math.copysign(1.0, values.pop(2)) == -1.0

values = [None] * 65
values[63] = 1.5
values.append(float('nan'))
values.extend([None, -0.0])
values *= 2
values.reverse()
assert math.copysign(1.0, values[0]) == -1.0
assert values[1] is None and math.isnan(values[2])
assert values[3] is None and values[4] == 1.5
assert values[0:2] == values[68:70]
values[1:4] = [1.5, None]
assert values[1:4] == [1.5, None, 1.5]

for other in (1, True, 2**63, 'a'):
    values = [0.25, None]
    values.append(other)
    assert type(values[0]) is float and type(values[2]) is type(other)
    assert values == [0.25, None, other]
