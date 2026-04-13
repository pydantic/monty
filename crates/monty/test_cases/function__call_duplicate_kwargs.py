def f(**kwargs):
    pass


f(**{'x': 1}, **{'x': 2})
# Raise=TypeError("f() got multiple values for keyword argument 'x'")
