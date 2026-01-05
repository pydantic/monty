# mode: iter
# Test that assigning to a frozen dataclass raises AttributeError
point = make_point()
point.x = 10
# Raise=AttributeError("'Point' object attribute 'x' is read-only")
