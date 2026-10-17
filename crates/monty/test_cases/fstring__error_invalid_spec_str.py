# /// monty
# xfail = ['cpython']
# ///
# invalid format specifier for string (detected at parse time)
f'{"hello":abc}'
# Raise=SyntaxError("Invalid format specifier 'abc'")
