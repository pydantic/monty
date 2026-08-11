# Tests dynamic keyword arguments for print()

# === Dynamic sep via **kwargs ===
dynamic_sep = 's' + 'e' + 'p'
result = print('left', 'right', **{dynamic_sep: '-'})
assert result is None


# === Dynamic end via **kwargs ===
dynamic_end = 'e' + 'n' + 'd'
result2 = print('line', **{dynamic_end: ''})
assert result2 is None
