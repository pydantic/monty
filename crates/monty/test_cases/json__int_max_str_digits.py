import json

# === loads accepts integers at the digit limit ===
load_ok = '1' * 4300
assert json.loads(load_ok) == int(load_ok), 'loads should accept 4300-digit integers'

# === loads rejects oversized decimal integers ===
try:
    json.loads('1' * 4301)
    assert False, 'loads should reject integers that exceed INT_MAX_STR_DIGITS'
except ValueError as exc:
    msg = str(exc)
    assert msg.startswith('Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits'), (
        f'loads digit-limit error message mismatch: {msg}'
    )

# === dumps accepts integers at the digit limit ===
dump_ok = 10**4299
assert json.dumps(dump_ok) == str(dump_ok), 'dumps should accept 4300-digit integers'

# === dumps rejects oversized decimal integers ===
try:
    json.dumps(10**4300)
    assert False, 'dumps should reject integers that exceed INT_MAX_STR_DIGITS'
except ValueError as exc:
    msg = str(exc)
    assert msg.startswith('Exceeds the limit (4300 digits) for integer string conversion'), (
        f'dumps digit-limit error message mismatch: {msg}'
    )
