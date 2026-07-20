# A traceback from a *decorated* class body locates the frame at the `class`
# keyword, not the first decorator. Three things that could misdirect the search
# for that keyword are present: a decorator argument holding the literal text
# `class`, a comment mentioning `class` between the decorators and the header,
# and irregular header spacing.
def tag(label):
    def deco(cls):
        return cls

    return deco


@tag('class Fake:')
@tag('x')
# class: a comment naming class, closer to the header than the keyword itself
class   C:
    a = 1
    b = 1 / 0


"""
TRACEBACK:
Traceback (most recent call last):
  File "decorator__class_body_traceback.py", line 16, in <module>
    class   C:
        a = 1
        b = 1 / 0
  File "decorator__class_body_traceback.py", line 18, in C
    b = 1 / 0
        ~~~~~
ZeroDivisionError: division by zero
"""
