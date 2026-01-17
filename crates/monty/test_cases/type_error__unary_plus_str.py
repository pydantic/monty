# unary plus on heap-allocated string should raise TypeError
# str() creates a heap-allocated string, triggering ref count check
+str(42)
# Raise=TypeError("bad operand type for unary +: 'str'")
