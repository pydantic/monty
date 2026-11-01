original = ''.join(['prefix-', 'a' * 100])
old_hash = hash(original)
lookup = {original: 'present'}
first = original + 'first'
second = first + 'second'
branch = original + 'branch'
assert original == 'prefix-' + 'a' * 100
assert first == original + 'first'
assert second == original + 'firstsecond'
assert branch == original + 'branch'
assert hash(original) == old_hash
assert lookup['prefix-' + 'a' * 100] == 'present'

doubling = 'q' * 100
doubled = doubling + doubling
assert doubling == 'q' * 100
assert doubled == 'q' * 200
doubling_unicode = 'é🙂' * 50
assert doubling_unicode + doubling_unicode == 'é🙂' * 100
assert doubling_unicode == 'é🙂' * 50

backing = 'abcdef' * 30
tail = backing + 'x'
overlap = backing[20:120]
joined = tail + overlap
assert overlap == ('abcdef' * 30)[20:120]
assert joined == 'abcdef' * 30 + 'x' + ('abcdef' * 30)[20:120]
assert tail == 'abcdef' * 30 + 'x'

iterator = iter(original)
expanded = second + 'x' * 1000
assert ''.join(iterator) == original
assert expanded == second + 'x' * 1000

iterated = 'ab' * 100
for character in iterated:
    iterated = iterated + character
assert iterated == 'ab' * 200

text = ''.join(['abé🙂' for _ in range(50)])
alias = text
view = text[10:]
for i in range(100):
    text = text + ('é' if i % 2 else 'x')
assert alias == 'abé🙂' * 50
assert view == alias[10:]
assert text == alias + 'xé' * 50
assert alias + alias == 'abé🙂' * 100

# The index callback grows the receiver's backing while a protected string handle is active.
source = 'a' * 100 + 'z'
saved_hash = hash(source)
class GrowDuringIndex:
    def __index__(self):
        global expanded
        expanded = source + 'é' * 100
        return 100
assert source[GrowDuringIndex()] == 'z'
assert source == 'a' * 100 + 'z'
assert hash(source) == saved_hash
assert expanded == source + 'é' * 100
assert len(expanded) == 201

def leave_cycle(value):
    view = value + 'tail'
    cycle = [view]
    cycle.append(cycle)

live = 'storage' * 30
leave_cycle(live)
for i in range(1000):
    temporary = [i]
assert live == 'storage' * 30

source = None
first = None
original = None
assert second == 'prefix-' + 'a' * 100 + 'firstsecond'
assert branch == 'prefix-' + 'a' * 100 + 'branch'
