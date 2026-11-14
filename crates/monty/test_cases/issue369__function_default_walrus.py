def name_0():
    def name_3(*, name_1=(name_5 := 0)):
        nonlocal name_5
        name_5 += 1

    name_3()
    assert name_5 == 1


name_0()
