# Parses the ~2 KB `medium_response.json` 1,000 times, amortizing per-run overhead.
# bench: data=../medium_response.json

import json

r = 0
for _ in range(1_000):
    r = len(json.loads(DATA))
assert r == 2
