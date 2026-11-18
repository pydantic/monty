import re

text = ' '.join(f'user{i}@example{i % 13}.com id={i:06d};' for i in range(8_000))
emails = re.findall(r'[a-z0-9]+@[a-z0-9]+\.com', text)
assert len(emails) == 8_000
ids = [int(m.group(1)) for m in re.finditer(r'id=(\d+);', text)]
assert sum(ids) == 31996000
assert len(re.sub(r'\d+', '#', text)) < len(text)
assert len(re.split(r';\s*', text)) == 8_001
