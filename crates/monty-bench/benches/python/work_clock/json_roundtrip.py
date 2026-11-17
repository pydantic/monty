import json

rows = [{'id': i, 'name': f'user-{i}', 'score': i / 7, 'tags': ['a', 'b', str(i % 5)], 'ok': i % 2 == 0} for i in range(8_000)]
text = json.dumps(rows)
assert len(text) == 790959
back = json.loads(text)
assert back == rows
assert json.loads(json.dumps(back, indent=2))[-1]['id'] == 7_999
