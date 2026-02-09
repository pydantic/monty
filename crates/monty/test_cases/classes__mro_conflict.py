# === C3 MRO conflict ===
class A:
    pass


class B(A):
    pass


# This should raise TypeError: A before B contradicts B's MRO (B -> A -> object)
class C(A, B):
    pass


"""
TRACEBACK:
Traceback (most recent call last):
  File "classes__mro_conflict.py", line 11, in <module>
    class C(A, B):
        pass
TypeError: Cannot create a consistent method resolution order (MRO) for bases A, B
"""
