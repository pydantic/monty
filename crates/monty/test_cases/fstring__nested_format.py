a = 1
b = 2
c = '04d'
assert f'{a:{b:{c}}}' == '01'
assert f'{a:{b:0{4}d}}' == '01'
assert f'{42:{len(f"{2}")}}' == '42'

events = []


def take(label, value):
    events.append(label)
    return value


assert f'{take("outer", 1):{take("middle", 2):{take("inner", "04d")}}}' == '01'
assert events == ['outer', 'middle', 'inner']

width = 2
try:
    f'{1:{width=}}'
except ValueError:
    pass
else:
    assert False, 'the debug prefix must be part of the format specification'

try:
    f'{1:{2:{4}d}}'
except ValueError:
    pass
else:
    assert False, 'the nested width must be formatted before formatting the outer value'

for prefix in ['f', 't']:
    try:
        eval(prefix + '"{a:{b:{c:{d}}}}"')
    except SyntaxError:
        pass
    else:
        assert False, 'a third nested format specification must be rejected'
