"""Unit tests for zero-argument super() & related machinery."""

import copy
import textwrap


class A:
    def f(self):
        return 'A'

class B(A):
    def f(self):
        return super().f() + 'B'

class C(A):
    def f(self):
        return super().f() + 'C'

class D(C, B):
    def f(self):
        return super().f() + 'D'
    def cm(cls):
        return (cls, super().cm(), 'D')

class E(D):
    pass

class F(E):
    f = E.f

class G(A):
    pass


class TestSuper(TestCase):

    def test_basics_working(self):
        self.assertEqual(D().f(), 'ABCD')

    def test_class_getattr_working(self):
        self.assertEqual(D.f(D()), 'ABCD')

    def test_subclass_no_override_working(self):
        self.assertEqual(E().f(), 'ABCD')
        self.assertEqual(E.f(E()), 'ABCD')

    def test_unbound_method_transfer_working(self):
        self.assertEqual(F().f(), 'ABCD')
        self.assertEqual(F.f(F()), 'ABCD')

    def test_class_methods_still_working(self):
        self.assertEqual(A.cm(), (A, 'A'))
        self.assertEqual(A().cm(), (A, 'A'))
        self.assertEqual(G.cm(), (G, 'A'))
        self.assertEqual(G().cm(), (G, 'A'))

    def test_super_in_class_methods_working(self):
        d = D()
        self.assertEqual(d.cm(), (d, (D, (D, (D, 'A'), 'B'), 'C'), 'D'))
        e = E()
        self.assertEqual(e.cm(), (e, (E, (E, (E, 'A'), 'B'), 'C'), 'D'))

    def test_super_with_closure(self):
        # Issue4360: super() did not work in a function that
        # contains a closure
        class E(A):
            def f(self):
                def nested():
                    self
                return super().f() + 'E'

        self.assertEqual(E().f(), 'AE')

    def test_various___class___pathologies(self):
        # See issue #12370
        class X(A):
            def f(self):
                return super().f()
            __class__ = 413
        x = X()
        self.assertEqual(x.f(), 'A')
        self.assertEqual(x.__class__, 413)
        class X:
            x = __class__
            def f():
                __class__
        self.assertIs(X.x, type(self))

    def test___class___instancemethod(self):
        # See issue #14857
        class X:
            def f(self):
                return __class__
        self.assertIs(X().f(), X)

    def test_obscure_super_errors(self):
        def f():
            super()
        with self.assertRaisesRegex(RuntimeError, r"no arguments"):
            f()

        class C:
            def f():
                super()
        with self.assertRaisesRegex(RuntimeError, r"no arguments"):
            C.f()

    def test_cell_as_self(self):
        class X:
            def meth(self):
                super()

        def f():
            k = X()
            def g():
                return k
            return g
        c = f().__closure__[0]
        self.assertRaises(TypeError, X.meth, c)

    def test_super_init_leaks(self):
        # Issue #26718: super.__init__ leaked memory if called multiple times.
        # This will be caught by regrtest.py -R if this leak.
        # NOTE: Despite the use in the test a direct call of super.__init__
        # is not endorsed.
        sp = super(float, 1.0)
        for i in range(1000):
            super.__init__(sp, int, i)

    def test_super_argcount(self):
        with self.assertRaisesRegex(TypeError, "expected at most"):
            super(int, int, int)

    def test_super_argtype(self):
        with self.assertRaisesRegex(TypeError, "argument 1 must be a type"):
            super(1, int)

    def test_shadowed_global(self):
        source = textwrap.dedent(
            """
            class super:
                msg = "truly super"

            class C:
                def method(self):
                    return super().msg
            """,
        )
        namespace = {}
        exec(source, namespace)
        self.assertEqual(namespace['C']().method(), "truly super")

    def test_shadowed_local(self):
        class super:
            msg = "quite super"

        class C:
            def method(self):
                return super().msg

        self.assertEqual(C().method(), "quite super")

    def test_shadowed_dynamic(self):
        class MySuper:
            msg = "super super"

        class C:
            def method(self):
                return super().msg

        global super
        original = super
        super = MySuper
        try:
            self.assertEqual(C().method(), "super super")
        finally:
            super = original

    def test_shadowed_dynamic_two_arg(self):
        call_args = []
        class MySuper:
            def __init__(self, *args):
                call_args.append(args)
            msg = "super super"

        class C:
            def method(self):
                return super(1, 2).msg

        global super
        original = super
        super = MySuper
        try:
            self.assertEqual(C().method(), "super super")
            self.assertEqual(call_args, [(1, 2)])
        finally:
            super = original

    def test_attribute_error(self):
        class C:
            def method(self):
                return super().msg

        with self.assertRaisesRegex(AttributeError, "'super' object has no attribute 'msg'"):
            C().method()

    def test_bad_first_arg(self):
        class C:
            def method(self):
                return super(1, self).method()

        with self.assertRaisesRegex(TypeError, "argument 1 must be a type"):
            C().method()

    def test_supercheck_fail(self):
        class C:
            def method(self, type_, obj):
                return super(type_, obj).method()

        c = C()
        err_msg = (
            r"super\(type, obj\): obj \({} {}\) is not "
            r"an instance or subtype of type \({}\)."
        )

        cases = (
            (int, c, int.__name__, C.__name__, "instance of"),
            # obj is instance of type
            (C, list(), C.__name__, list.__name__, "instance of"),
            # obj is type itself
            (C, list, C.__name__, list.__name__, "type"),
        )

        for case in cases:
            with self.subTest(case=case):
                type_, obj, type_str, obj_str, instance_or_type = case
                regex = err_msg.format(instance_or_type, obj_str, type_str)

                with self.assertRaisesRegex(TypeError, regex):
                    c.method(type_, obj)

    def test_super___class__(self):
        class C:
            def method(self):
                return super().__class__

        self.assertEqual(C().method(), super)

    def test_super_subclass___class__(self):
        class mysuper(super):
            pass

        class C:
            def method(self):
                return mysuper(C, self).__class__

        self.assertEqual(C().method(), mysuper)

    def test_unusual_getattro(self):
        from test.support import import_helper

        class MyType(type):
            pass

        def test(name):
            mytype = MyType(name, (MyType,), {})
            super(MyType, type(mytype)).__setattr__(mytype, "bar", 1)
            self.assertEqual(mytype.bar, 1)

        _testinternalcapi = import_helper.import_module("_testinternalcapi")
        for _ in range(_testinternalcapi.SPECIALIZATION_THRESHOLD):
            test("foo1")

    def test_reassigned_new(self):
        from test.support import import_helper

        class A:
            def __new__(cls):
                pass

            def __init_subclass__(cls):
                if "__new__" not in cls.__dict__:
                    cls.__new__ = cls.__new__

        class B(A):
            pass

        class C(B):
            def __new__(cls):
                return super().__new__(cls)

        _testinternalcapi = import_helper.import_module("_testinternalcapi")
        for _ in range(_testinternalcapi.SPECIALIZATION_THRESHOLD):
            C()

    def test___class___modification_multithreaded(self):
        """ Note: this test isn't actually testing anything on its own.
        It requires a sys audithook to be set to crash on older Python.
        This should be the case anyways as our test suite sets
        an audit hook.
        """
        import threading

        class Foo:
            pass

        class Bar:
            pass

        thing = Foo()
        def work():
            foo = thing
            for _ in range(200):
                foo.__class__ = Bar
                type(foo)
                foo.__class__ = Foo
                type(foo)


        threads = []
        for _ in range(6):
            thread = threading.Thread(target=work)
            thread.start()
            threads.append(thread)

        for thread in threads:
            thread.join()

    def test_special_methods(self):
        for e in E(), E:
            s = super(C, e)
            self.assertEqual(s.__reduce__, e.__reduce__)
            self.assertEqual(s.__reduce_ex__, e.__reduce_ex__)
            self.assertEqual(s.__getstate__, e.__getstate__)
            self.assertNotHasAttr(s, '__getnewargs__')
            self.assertNotHasAttr(s, '__getnewargs_ex__')
            self.assertNotHasAttr(s, '__setstate__')
            self.assertNotHasAttr(s, '__copy__')
            self.assertNotHasAttr(s, '__deepcopy__')

    def test_pickling(self):
        import pickle

        e = E()
        e.x = 1
        s = super(C, e)
        for proto in range(pickle.HIGHEST_PROTOCOL + 1):
            with self.subTest(proto=proto):
                u = pickle.loads(pickle.dumps(s, proto))
                self.assertEqual(u.f(), s.f())
                self.assertIs(type(u), type(s))
                self.assertIs(type(u.__self__), E)
                self.assertEqual(u.__self__.x, 1)
                self.assertIs(u.__thisclass__, C)
                self.assertIs(u.__self_class__, E)

        s = super(C, E)
        for proto in range(pickle.HIGHEST_PROTOCOL + 1):
            with self.subTest(proto=proto):
                u = pickle.loads(pickle.dumps(s, proto))
                self.assertEqual(u.cm(), s.cm())
                self.assertEqual(u.f, s.f)
                self.assertIs(type(u), type(s))
                self.assertIs(u.__self__, E)
                self.assertIs(u.__thisclass__, C)
                self.assertIs(u.__self_class__, E)

    def test_shallow_copying(self):
        s = super(C, E())
        self.assertIs(copy.copy(s), s)
        s = super(C, E)
        self.assertIs(copy.copy(s), s)

    def test_deep_copying(self):
        e = E()
        e.x = [1]
        s = super(C, e)
        u = copy.deepcopy(s)
        self.assertEqual(u.f(), s.f())
        self.assertIs(type(u), type(s))
        self.assertIsNot(u, s)
        self.assertIs(type(u.__self__), E)
        self.assertIsNot(u.__self__, e)
        self.assertIsNot(u.__self__.x, e.x)
        self.assertEqual(u.__self__.x, [1])
        self.assertIs(u.__thisclass__, C)
        self.assertIs(u.__self_class__, E)

        s = super(C, E)
        u = copy.deepcopy(s)
        self.assertEqual(u.cm(), s.cm())
        self.assertEqual(u.f, s.f)
        self.assertIsNot(u, s)
        self.assertIs(type(u), type(s))
        self.assertIs(u.__self__, E)
        self.assertIs(u.__thisclass__, C)
        self.assertIs(u.__self_class__, E)


for name in (
    'test_basics_working',
    'test_class_getattr_working',
    'test_subclass_no_override_working',
    'test_unbound_method_transfer_working',
    'test_super_with_closure',
    'test_various___class___pathologies',
    'test___class___instancemethod',
    'test_obscure_super_errors',
    'test_super_argcount',
    'test_super_argtype',
    'test_shadowed_global',
    'test_shadowed_local',
    'test_shadowed_dynamic',
    'test_shadowed_dynamic_two_arg',
    'test_attribute_error',
    'test_bad_first_arg',
    'test_supercheck_fail',
    'test_super___class__',
):
    getattr(TestSuper(), name)()
