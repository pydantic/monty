# A traceback from a *decorated* class body locates the frame at the `class`
# keyword, not the first decorator. The decorator argument holds the literal
# text `class` and the header spacing is irregular: neither may fool the scan.
def tag(label):
    def deco(cls):
        return cls

    return deco


@tag('class Fake:')
@tag('x')
class   C:
    a = 1
    b = 1 / 0


"""
TRACEBACK:
Traceback (most recent call last):
  File "decorator__class_body_traceback.py", line 13, in <module>
    class   C:
        a = 1
        b = 1 / 0
  File "decorator__class_body_traceback.py", line 15, in C
    b = 1 / 0
        ~~~~~
ZeroDivisionError: division by zero
"""
