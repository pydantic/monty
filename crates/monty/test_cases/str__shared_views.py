source = ''.join(['0123456789abcdef' for _ in range(100)])
first = source[100:1500]
second = first[100:1300]
assert second == source[200:1400]
assert first is not source
assert first is not source[100:1500]
keys = {first: 'first', second: 'second'}
assert keys[source[100:1500]] == 'first'
assert second in {source[200:1400]}

source = None
first = None
assert second == ('0123456789abcdef' * 100)[200:1400]
assert second[::3] == ('0123456789abcdef' * 100)[200:1400:3]
assert second[::-1] == ('0123456789abcdef' * 100)[200:1400][::-1]
assert second[-800:-100] == ('0123456789abcdef' * 100)[600:1300]
assert second[800:100] == ''

for width in [0, 1, 15, 16, 64, 65, 400, 401, 800]:
    text = ''.join(['x' for _ in range(1600)])
    value = text[10:10 + width]
    text = None
    assert value == 'x' * width

unicode_source = ''.join(['aé中🙂' for _ in range(200)])
unicode_view = unicode_source[50:700]
unicode_nested = unicode_view[40:600]
assert unicode_view == ('aé中🙂' * 200)[50:700]
assert unicode_nested == ('aé中🙂' * 200)[90:650]
assert len(unicode_nested) == 560
assert unicode_nested[0] == '中'
unicode_source = None
unicode_view = None
assert unicode_nested[::-7] == ('aé中🙂' * 200)[90:650][::-7]

mixed_source = 'é' + ('a' * 1000) + '🙂'
ascii_view = mixed_source[1:901]
mixed_source = None
assert len(ascii_view) == 900
assert ascii_view[899] == 'a'
assert ascii_view[::-1] == 'a' * 900
assert hash(ascii_view) == hash('a' * 900)
for width in [0, 1, 7, 8, 15, 16, 65]:
    assert ('é' * 100)[1:1 + width] == 'é' * width

original = ''.join(['abcdef0123456789' for _ in range(100)])
view = original[100:1500]
alias = original
old_hash = hash(view)
original += 'suffix'
assert alias == 'abcdef0123456789' * 100
assert view == alias[100:1500]
assert hash(view) == old_hash
assert view.upper().lower() == view
assert view.encode().decode() == view

# Drop many overlapping siblings in a deterministic order while keeping dictionary keys alive.
siblings = [alias[i:1500 - i] for i in range(200)]
expected = [s + '' for s in siblings]
for i in range(200):
    index = (i * 97) % 200
    assert siblings[index] == expected[index]
    siblings[index] = None
assert keys[('0123456789abcdef' * 100)[200:1400]] == 'second'
