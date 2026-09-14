# The locals snapshot an implicit exec/eval takes inside a function is
# released when the snippet frame pops, including when the snippet raises.
obj = [1]


def f():
    local = obj
    exec('x = [1]')
    exec('x = [2]')
    try:
        exec('raise ValueError')
    except ValueError:
        pass
    return eval('local')


res = f()
# ref-counts={'obj': 2, 'res': 2}
