# GC should free references held by cycles when they are cleared
external = {}

# this is a clumsy way to build a cycle due to Monty not supporting `del` yet
values = [[external], [external]]
values[0].append(values[1])
values[1].append(values[0])

values.clear()

# ref-counts={'external': 1, 'values': 1}
