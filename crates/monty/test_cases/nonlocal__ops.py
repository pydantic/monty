# === Basic nonlocal read/write ===
def read_outer():
    x = 10

    def inner():
        return x  # reads from outer scope

    return inner()


assert read_outer() == 10


def write_outer():
    x = 10

    def inner():
        nonlocal x
        x = 20

    inner()
    return x


assert write_outer() == 20


# === Classic counter pattern ===
def make_counter():
    count = 0

    def increment():
        nonlocal count
        count = count + 1
        return count

    return increment


counter2 = make_counter()
assert counter2() == 1
assert counter2() == 2
assert counter2() == 3


# === Implicit capture (read without nonlocal) ===
def implicit_capture():
    a = 10
    b = 20

    def inner():
        return a + b  # reads both from outer

    return inner()


assert implicit_capture() == 30


# === Pass-through nesting ===
def pass_through():
    x = 1

    def middle():
        nonlocal x
        x = x + 10

        def inner():
            nonlocal x
            x = x + 100
            return x

        r1 = inner()  # returns 111
        r2 = x  # x is now 111
        return r1 + r2  # 222

    return middle()


assert pass_through() == 222


# === Deep nesting (3 levels) ===
def deep_nesting():
    x = 1

    def level2():
        nonlocal x
        x = x + 10

        def level3():
            nonlocal x
            x = x + 100
            return x

        return level3()

    return level2()


assert deep_nesting() == 111


# === Deep nesting (4 levels) ===
def deep_pass_through():
    val = 1

    def level1():
        nonlocal val
        val = val + 1

        def level2():
            nonlocal val
            val = val + 10

            def level3():
                nonlocal val
                val = val + 100
                return val

            return level3()

        return level2()

    result = level1()  # val: 1 -> 2 -> 12 -> 112
    return (result, val)


assert deep_pass_through() == (112, 112)


# === Multiple independent cells ===
def multiple_cells():
    a = 1
    b = 10
    c = 100

    def modify_a():
        nonlocal a
        a = a + 1
        return a

    def modify_b():
        nonlocal b
        b = b + 10
        return b

    def modify_c():
        nonlocal c
        c = c + 100
        return c

    def read_all():
        return a + b + c

    r1 = modify_b()  # b = 20
    r2 = modify_a()  # a = 2
    r3 = modify_c()  # c = 200
    r4 = read_all()  # 2 + 20 + 200 = 222
    return (r1, r2, r3, r4)


assert multiple_cells() == (20, 2, 200, 222)


# === Shared cell (getter/setter pattern) ===
def shared_cell():
    x = 0

    def getter():
        return x

    def setter(v):
        nonlocal x
        x = v

    return (getter, setter)


pair = shared_cell()
getter = pair[0]
setter = pair[1]
assert getter() == 0
setter(42)
assert getter() == 42


# === Shared multiple vars ===
def shared_multiple_vars():
    x = 0
    y = 0

    def add_to_x(n):
        nonlocal x
        x = x + n
        return x

    def add_to_y(n):
        nonlocal y
        y = y + n
        return y

    def swap():
        nonlocal x, y
        tmp = x
        x = y
        y = tmp
        return (x, y)

    def get_both():
        return (x, y)

    return (add_to_x, add_to_y, swap, get_both)


ops = shared_multiple_vars()
add_x = ops[0]
add_y = ops[1]
swap = ops[2]
get = ops[3]
add_x(5)  # x=5
add_y(10)  # y=10
add_x(3)  # x=8
swap()  # x=10, y=8
assert get() == (10, 8)


# === Local and captured ===
def local_and_captured():
    x = 1

    def inner():
        nonlocal x
        x = x + x
        return x

    before = x  # 1
    middle = inner()  # 2
    after = x  # 2
    final = inner()  # 4
    return (before, middle, after, final, x)


assert local_and_captured() == (1, 2, 2, 4, 4)


# === Mixing global and nonlocal ===
g1 = 100


def global_and_nonlocal():
    x = 1

    def inner():
        global g1
        nonlocal x
        g1 = g1 + 1
        x = x + 10
        return g1 + x

    return inner()


assert global_and_nonlocal() == 112


# === Closure with global and nonlocal ===
g2 = 1000


def make_closure_global():
    x = 1

    def closure():
        global g2
        nonlocal x
        result = g2 + x
        g2 = g2 + 1
        x = x + 10
        return result

    return closure


c = make_closure_global()
r1 = c()  # returns 1001
r2 = c()  # returns 1012
r3 = c()  # returns 1023
assert (r1, r2, r3, g2) == (1001, 1012, 1023, 1003)


# === Closure creates closure ===
def outer_factory():
    outer_val = 10

    def inner_factory():
        nonlocal outer_val
        inner_val = outer_val

        def innermost():
            nonlocal inner_val
            inner_val = inner_val + 1
            return inner_val

        outer_val = outer_val + 100
        return innermost

    return inner_factory


factory = outer_factory()
closure1 = factory()  # inner_val=10, outer_val->110
closure2 = factory()  # inner_val=110, outer_val->210
r1 = closure1()  # 11
r2 = closure1()  # 12
r3 = closure2()  # 111
r4 = closure1()  # 13
assert (r1, r2, r3, r4) == (11, 12, 111, 13)


# === Augmented assignment with nonlocal ===
def augmented_assign():
    x = 10

    def inner():
        nonlocal x
        x += 5

    inner()
    return x


assert augmented_assign() == 15


# === Cell contains closure ===
def cell_contains_closure():
    y = 100

    def inner():
        return y

    x = inner  # x holds closure, x is also a cell var

    def get_x():
        nonlocal x
        return x

    f = get_x()
    return f()


assert cell_contains_closure() == 100
