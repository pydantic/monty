# call-external
# BUG: External calls in closures cannot access captured variables
# When an external call is inside a closure, it fails to access free variables
# Error: "cannot access free variable 'x' where it is not associated with a value"


def make_adder(x):
    def adder(y):
        return add_ints(x, y)

    return adder


add5 = make_adder(5)
result = add5(10)
assert result == 15, 'ext call in closure accessing captured var'
