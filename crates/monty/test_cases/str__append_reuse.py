def build(count):
    text = ''.join(['initial-', '0123456789abcdef'])
    for i in range(count):
        text += str(i % 10)
    return text

assert build(1000) == 'initial-0123456789abcdef' + '0123456789' * 100

length_controlled = build(20)
while len(length_controlled) < 1000:
    length_controlled += 'x'
assert len(length_controlled) == 1000
length_controlled += '🙂'
assert len(length_controlled) == 1001
assert length_controlled[-1] == '🙂'

text = ''.join(['initial-', '0123456789abcdef'])
old_hash = hash(text)
for i in range(500):
    text += 'é🙂' if i % 2 else 'a'
assert text == 'initial-0123456789abcdef' + 'aé🙂' * 250
assert hash(text) == hash('initial-0123456789abcdef' + 'aé🙂' * 250)
assert len(text) == 24 + 750

original = ''.join(['a long ', 'immutable string' * 30])
alias = original
lookup = {original: 'present'}
snapshots = []
views = []
for i in range(100):
    snapshots.append(original)
    views.append(original[10:])
    original += str(i)
assert alias == 'a long ' + 'immutable string' * 30
assert lookup[alias] == 'present'
for index in range(100):
    expected = alias + ''.join(str(i) for i in range(index))
    assert snapshots[index] == expected
    assert views[index] == expected[10:]

middle = alias[20:-20]
old_middle = middle + ''
middle += 'suffix'
assert middle == old_middle + 'suffix'
assert alias == 'a long ' + 'immutable string' * 30

text = build(100)
expected = text + text
text += text
assert text == expected

# RHS evaluation can replace the target binding before the augmented assignment stores its result.
text = build(20)
old = text + ''
def replace_target():
    global text
    text = 'replacement'
    return 'suffix'
text += replace_target()
assert text == old + 'suffix'

class RejectStore:
    def __init__(self):
        self.value = build(20)
    def __getitem__(self, index):
        return self.value
    def __setitem__(self, index, value):
        raise ValueError('rejected')

box = RejectStore()
before = box.value + ''
try:
    box[0] += 'suffix'
except ValueError:
    pass
else:
    assert False
assert box.value == before

def closure_builder():
    text = build(20)
    def append():
        nonlocal text
        text += 'suffix'
    append()
    return text
assert closure_builder() == build(20) + 'suffix'
