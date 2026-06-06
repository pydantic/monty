# cpython-main-module

# === Script-style module dunders ===
assert __name__ == '__main__', 'module __name__ is __main__'
assert __name__ is __name__, 'module __name__ is a stable interned/global string'
assert __debug__ is True, 'module __debug__ is true'


# === Main guard idiom ===
ran_main_guard = False
if __name__ == '__main__':
    ran_main_guard = True

assert ran_main_guard is True, 'main guard executes for top-level Monty code'


# === Reads from function global scope ===
def module_name_from_function():
    return __name__


assert module_name_from_function() == '__main__', 'function global read resolves module __name__'
