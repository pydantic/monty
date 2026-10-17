# /// monty
# xfail = ['cpython']
# ///
# invalid format specifier with trailing characters (detected at parse time)
f'{1:10xyz}'
# Raise=SyntaxError("Invalid format specifier '10xyz'")
