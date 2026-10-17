# /// monty
# [bench]
# data = '../medium_response.json'
# ///
# Serializes the parsed `medium_response.json` 1,000 times.

import json

obj = json.loads(DATA)
r = 0
for _ in range(1_000):
    r = len(json.dumps(obj))
assert r == 1815
