text = ('lorem ipsum dolor sit amet ' * 40_000) + 'needle'
assert text.find('needle') == 1_080_000
assert text.count('dolor') == 40_000
assert len(text.replace('ipsum', 'IPSUM')) == len(text)
assert len(text.split(' ')) == 200001
assert text.encode().decode() == text
