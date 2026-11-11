# Adapted from Flying Circus's nearby reads: alternate signed indices with small backtracks.
sequence = 'aé中🐍' * 4096
total = 0

for i in range(4096):
    index = 4096 + i // 2 + i % 2
    total += ord(sequence[index]) + ord(sequence[index - 1 - 16384])

assert total == 303833088
