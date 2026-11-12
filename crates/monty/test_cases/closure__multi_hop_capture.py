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


# === Deep pass-through nesting ===
# Scope analysis once re-walked every deeper function per level, so 24 levels took hours to compile.
def deep_0():
    x = 'deep'

    def deep_1():
        def deep_2():
            def deep_3():
                def deep_4():
                    def deep_5():
                        def deep_6():
                            def deep_7():
                                def deep_8():
                                    def deep_9():
                                        def deep_10():
                                            def deep_11():
                                                def deep_12():
                                                    def deep_13():
                                                        def deep_14():
                                                            def deep_15():
                                                                def deep_16():
                                                                    def deep_17():
                                                                        def deep_18():
                                                                            def deep_19():
                                                                                def deep_20():
                                                                                    def deep_21():
                                                                                        def deep_22():
                                                                                            def deep_23():
                                                                                                return x

                                                                                            return deep_23()

                                                                                        return deep_22()

                                                                                    return deep_21()

                                                                                return deep_20()

                                                                            return deep_19()

                                                                        return deep_18()

                                                                    return deep_17()

                                                                return deep_16()

                                                            return deep_15()

                                                        return deep_14()

                                                    return deep_13()

                                                return deep_12()

                                            return deep_11()

                                        return deep_10()

                                    return deep_9()

                                return deep_8()

                            return deep_7()

                        return deep_6()

                    return deep_5()

                return deep_4()

            return deep_3()

        return deep_2()

    return deep_1()


assert deep_0() == 'deep'
