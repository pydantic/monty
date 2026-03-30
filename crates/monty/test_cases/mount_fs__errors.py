# mount-fs
from pathlib import Path

# root is injected by the test runner

# === FileNotFoundError on read_text of nonexistent ===
try:
    (root / 'nonexistent.txt').read_text()
    assert False, 'expected FileNotFoundError on read_text'
except FileNotFoundError:
    pass

# === FileNotFoundError on read_bytes of nonexistent ===
try:
    (root / 'nonexistent.bin').read_bytes()
    assert False, 'expected FileNotFoundError on read_bytes'
except FileNotFoundError:
    pass

# === FileNotFoundError on unlink of nonexistent ===
try:
    (root / 'nonexistent.txt').unlink()
    assert False, 'expected FileNotFoundError on unlink'
except FileNotFoundError:
    pass

# === FileNotFoundError on stat of nonexistent ===
try:
    (root / 'nonexistent.txt').stat()
    assert False, 'expected FileNotFoundError on stat'
except FileNotFoundError:
    pass

# === Error on mkdir without parents when parent missing ===
try:
    (root / 'missing_parent' / 'child').mkdir()
    assert False, 'expected error on mkdir without parents'
except (FileNotFoundError, OSError):
    pass

# === FileExistsError on mkdir of existing dir without exist_ok ===
try:
    (root / 'subdir').mkdir()
    assert False, 'expected FileExistsError on mkdir existing'
except FileExistsError:
    pass

# === Error on rmdir of non-empty directory ===
try:
    (root / 'subdir').rmdir()
    assert False, 'expected error on rmdir non-empty'
except OSError:
    pass

# === FileNotFoundError on rmdir of nonexistent ===
try:
    (root / 'nonexistent_dir').rmdir()
    assert False, 'expected FileNotFoundError on rmdir nonexistent'
except (FileNotFoundError, OSError):
    pass
