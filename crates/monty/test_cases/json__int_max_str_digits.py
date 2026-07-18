import json

# === loads accepts integers at the digit limit ===
load_ok = '1' * 4300
assert json.loads(load_ok) == int(load_ok)

# === loads rejects oversized decimal integers ===
try:
    json.loads('1' * 4301)
    assert False, 'loads should reject integers that exceed INT_MAX_STR_DIGITS'
except ValueError as exc:
    msg = str(exc)
    assert msg.startswith('Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits'), (
        f'loads digit-limit error message mismatch: {msg}'
    )

# === loads rejects multi-million-digit integers without growing a BigInt ===
# Regression test: the parser must reject by digit count before any BigInt
# allocation, so this completes in milliseconds rather than spending CPU/memory
# proportional to the input size.
huge = '1' * 1_000_000
try:
    json.loads(huge)
    assert False, 'loads should reject multi-million-digit integers'
except ValueError as exc:
    msg = str(exc)
    assert msg.startswith('Exceeds the limit (4300 digits) for integer string conversion: value has 1000000 digits'), (
        f'loads huge-int error message mismatch: {msg}'
    )

# === dumps accepts integers at the digit limit ===
dump_ok = 10**4299
assert json.dumps(dump_ok) == str(dump_ok)

# === dumps rejects oversized decimal integers ===
try:
    json.dumps(10**4300)
    assert False, 'dumps should reject integers that exceed INT_MAX_STR_DIGITS'
except ValueError as exc:
    msg = str(exc)
    assert msg.startswith('Exceeds the limit (4300 digits) for integer string conversion'), (
        f'dumps digit-limit error message mismatch: {msg}'
    )
