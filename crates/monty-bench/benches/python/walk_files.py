# /// monty
# [bench]
# mount = true
# ///
# A top-down `os.walk` over the mounted scan tree, counting files.

import os

count = 0
for _, _, files in os.walk(DATA):
    count += len(files)
assert count == 4_020
