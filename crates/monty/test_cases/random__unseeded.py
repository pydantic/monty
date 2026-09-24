# call-external
# Unseeded `random`: the generator seeds itself from OS entropy (on the first
# draw in Monty, at import in CPython), so only invariants can be asserted here.
import random

# === the module-level generator seeds itself on first use ===
x = random.random()
assert 0.0 <= x < 1.0
assert 0 <= random.getrandbits(8) < 256
assert 1 <= random.randint(1, 6) <= 6
assert random.choice(['only']) == 'only'
assert len(random.randbytes(3)) == 3

# === seed() with no argument reseeds from the host ===
assert random.seed() is None
assert random.seed(None) is None
assert 0.0 <= random.random() < 1.0

# === an unseeded instance seeds itself independently of the module ===
r = random.Random()
assert 0.0 <= r.random() < 1.0
assert 0 <= r.randrange(10) < 10
state = r.getstate()
assert len(state[1]) == 625
assert random.Random().getstate()[0] == 3
assert 0.0 <= random.Random(None).uniform(0, 1) < 1.0
