data = bytes(range(256)) * 8_000
assert len(data) == 2_048_000
assert data.count(b'\x00\x01') == 8_000
assert data.find(b'\xff\x00\x02') == -1
assert len(data.replace(b'\x10', b'\x10\x10')) == 2_056_000
assert data[::-1][0] == 255
assert sum(data[:100_000]) == 12742320
assert data[1000:2000].hex()[:4] == 'e8e9'
