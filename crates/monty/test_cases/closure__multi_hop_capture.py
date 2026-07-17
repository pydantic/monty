# === Two-hop capture through pass-through scope (regression for #469 / #477) ===
# `inner` reads `x` which is bound in `outer`; `middle` is a pass-through scope
# that doesn't reference `x` itself but must propagate the cell.
def two_hop():
    x = 1

    def middle():
        def inner():
            return x

        return inner()

    return middle()


assert two_hop() == 1


# === Three-hop capture: f -> g -> h -> i ===
def three_hop():
    x = 'deep'

    def g():
        def h():
            def i():
                return x

            return i()

        return h()

    return g()


assert three_hop() == 'deep'


# === Multiple captures across nested scopes ===
def multi_capture():
    a = 1
    b = 2

    def middle():
        def inner():
            return (a, b)

        return inner()

    return middle()


assert multi_capture() == (1, 2)


# === Pass-through scope reads the captured name itself too ===
def pass_through_reads():
    x = 10

    def middle():
        local_y = x + 1  # middle reads x directly

        def inner():
            return x + local_y  # inner also reads x (capture) and middle's local

        return inner()

    return middle()


assert pass_through_reads() == 10 + 10 + 1


# === Mixed nonlocal + implicit multi-hop ===
def nonlocal_through_pass_through():
    x = 100

    def middle():
        def inner():
            nonlocal x
            x = 200

        inner()
        return x

    return middle()


assert nonlocal_through_pass_through() == 200


# === Lambda captures through pass-through ===
def lambda_pass_through():
    x = 'lambda'

    def middle():
        return lambda: x

    return middle()()


assert lambda_pass_through() == 'lambda'


# === Multiple inner functions, only one captures ===
def selective_capture():
    x = 'captured'

    def middle():
        def reader():
            return x  # captures

        def non_reader():
            return 'unrelated'  # does not capture

        return reader() + ':' + non_reader()

    return middle()


assert selective_capture() == 'captured:unrelated'
