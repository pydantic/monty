# Regression test: list.extend() with a non-iterable should still raise TypeError
# with the correct message. This verifies that the list_extend opcode helper in
# collections.rs (for [*expr] literals) and the list.extend() method in
# types/list.rs are separate code paths that do not interfere with each other.
a = []
a.extend(1)
# Raise=TypeError("'int' object is not iterable")
