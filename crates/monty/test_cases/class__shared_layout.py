import copy


class Record:
    fallback = 'class value'

    def method(self):
        return self.x


p = Record()
q = Record()
p.x = [1]
p.y = 2
q.y = 20
q.x = [10]
assert p.x == [1] and q.x == [10]
assert p.y == 2 and q.y == 20
p.fallback = None
assert q.fallback == 'class value'
assert not hasattr(q, 'z')
p.z = 3
p.fifth = 5
p.sixth = 6
assert not hasattr(q, 'fifth')
assert p.method() == [1]
q.method = lambda: 42
assert q.method() == 42

shallow = copy.copy(p)
deep = copy.deepcopy(p)
assert shallow.x is p.x
assert deep.x == p.x and deep.x is not p.x
p.cycle = p
cyclic = copy.deepcopy(p)
assert cyclic.cycle is cyclic

# A computed name marks the class for dictionary storage, preserving old slots.
setattr(q, 'dynamic_' + str(123), [99])
assert p.sixth == 6 and q.x == [10]
p.y = 22
r = Record()
r.x = [100]
assert r.x == [100] and p.y == 22
assert getattr(q, 'dynamic_' + str(123)) == [99]
assert copy.deepcopy(q).method() == 42


class Wide:
    pass


wide = Wide()
wide.field_0 = 0
wide.field_1 = 1
wide.field_2 = 2
wide.field_3 = 3
wide.field_4 = 4
wide.field_5 = 5
wide.field_6 = 6
wide.field_7 = 7
wide.field_8 = 8
wide.field_9 = 9
wide.field_10 = 10
wide.field_11 = 11
wide.field_12 = 12
wide.field_13 = 13
wide.field_14 = 14
wide.field_15 = 15
wide.field_16 = 16
wide.field_17 = 17
wide.field_18 = 18
wide.field_19 = 19
wide.field_20 = 20
wide.field_21 = 21
wide.field_22 = 22
wide.field_23 = 23
wide.field_24 = 24
wide.field_25 = 25
wide.field_26 = 26
wide.field_27 = 27
wide.field_28 = 28
wide.field_29 = 29
wide.field_30 = 30
wide.field_31 = 31
wide.field_32 = 32
wide.field_33 = 33
wide.field_34 = 34
assert wide.field_0 == 0 and wide.field_34 == 34
wide_copy = copy.deepcopy(wide)
assert wide_copy.field_0 == 0 and wide_copy.field_34 == 34


class ConvertDuringCopy:
    def __deepcopy__(self, memo):
        # Another instance changes the class mode before the source's next write.
        setattr(other, 'computed_' + str(456), 456)
        source.last = 30
        return 10


class DuringCopy:
    pass


source = DuringCopy()
source.first = ConvertDuringCopy()
other = DuringCopy()
source.middle = 1
source.last = 3
copied = copy.deepcopy(source)
assert copied.first == 10 and copied.middle == 1 and copied.last == 30
