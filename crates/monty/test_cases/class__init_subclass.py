# Class definition keyword arguments and __init_subclass__ hook

# 1. Basic __init_subclass__ execution with keyword arguments
class PluginBase:
    registry = []

    def __init_subclass__(cls, plugin_name=None, **kwargs):
        super().__init_subclass__(**kwargs)
        cls.plugin_name = plugin_name
        cls.registry.append(cls)


class PluginA(PluginBase, plugin_name='alpha'):
    pass


class PluginB(PluginBase, plugin_name='beta'):
    pass


assert PluginA.plugin_name == 'alpha'
assert PluginB.plugin_name == 'beta'
assert PluginBase.registry == [PluginA, PluginB]

# 2. Base class definition does not call its own __init_subclass__
calls = []


class Tracker:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        calls.append(cls.__name__)


assert calls == []


class SubTracker(Tracker):
    pass


assert calls == ['SubTracker']

# 3. Multilevel cooperative __init_subclass__ with kwargs forwarding
log = []


class A:
    def __init_subclass__(cls, a=1, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(('A', cls.__name__, a))


class B(A):
    def __init_subclass__(cls, b=2, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(('B', cls.__name__, b))


assert log == [('A', 'B', 1)]


class C(B, a=10, b=20):
    pass


assert log == [('A', 'B', 1), ('A', 'C', 10), ('B', 'C', 20)]


# 4. Dynamic type() call with keyword arguments
class DynBase:
    def __init_subclass__(cls, tag=None, **kwargs):
        super().__init_subclass__(**kwargs)
        cls.tag = tag


DynSub = type('DynSub', (DynBase,), {}, tag='dynamic_plugin')
assert DynSub.tag == 'dynamic_plugin'

# 5. TypeError when class kwargs passed but no base defines __init_subclass__
no_kwarg_error = None
try:

    class Standalone(extra='val'):
        pass
except TypeError as e:
    no_kwarg_error = str(e)
assert no_kwarg_error == 'Standalone.__init_subclass__() takes no keyword arguments'

obj_kwarg_error = None
try:

    class ObjSub(object, extra='val'):
        pass
except TypeError as e:
    obj_kwarg_error = str(e)
assert obj_kwarg_error == 'ObjSub.__init_subclass__() takes no keyword arguments'

# 6. Direct calls to __init_subclass__ on object and user classes
assert object.__init_subclass__() is None


class EmptyBase:
    pass


assert EmptyBase.__init_subclass__() is None

direct_pos_error = None
try:
    object.__init_subclass__(1)
except TypeError as e:
    direct_pos_error = str(e)
assert direct_pos_error == 'object.__init_subclass__() takes no arguments (1 given)'

direct_kw_error = None
try:
    object.__init_subclass__(x=1)
except TypeError as e:
    direct_kw_error = str(e)
assert direct_kw_error == 'object.__init_subclass__() takes no keyword arguments'

base_pos_error = None
try:
    EmptyBase.__init_subclass__(1)
except TypeError as e:
    base_pos_error = str(e)
assert base_pos_error == 'EmptyBase.__init_subclass__() takes no arguments (1 given)'

base_kw_error = None
try:
    EmptyBase.__init_subclass__(x=1)
except TypeError as e:
    base_kw_error = str(e)
assert base_kw_error == 'EmptyBase.__init_subclass__() takes no keyword arguments'
