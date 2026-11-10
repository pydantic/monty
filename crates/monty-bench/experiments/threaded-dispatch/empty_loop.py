def hot():
    for _ in range(200_000):
        pass
    return 42
result = hot()
print(result)
