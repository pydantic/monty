# cpython-main-module

# === Script-style module dunders ===
assert __name__ == '__main__'
assert __name__ is __name__
assert __debug__ is True

# === __file__ is the absolute path of the script ===
from pathlib import Path

assert isinstance(__file__, str)
assert Path(__file__).is_absolute()
assert Path(__file__).name == 'module__dunders.py'


# === Main guard idiom ===
ran_main_guard = False
if __name__ == '__main__':
    ran_main_guard = True

assert ran_main_guard is True


# === Reads from function global scope ===
def module_name_from_function():
    return __name__


assert module_name_from_function() == '__main__'
