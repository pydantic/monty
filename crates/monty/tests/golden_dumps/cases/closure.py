def counter(start):
    def bump(step):
        return start + step

    return bump


bump = counter(10)
first = bump(1)
