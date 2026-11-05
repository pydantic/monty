use std::time::Duration;

use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, ExcType, MontyObject, MontyUuid, PrintWriter, ResourceLimits, ResourceTracker};

fn run(code: &str) {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap();
}

fn run_err(code: &str, expected: &str) {
    let err = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap_err();
    assert!(err.to_string().contains(expected), "actual error: {err}");
}

#[test]
fn parent_evaluates_once_before_class_body() {
    run(r"
events = []

class Parent:
    pass

def parent():
    events.append('parent')
    return Parent

def member():
    events.append('body')
    return 1

class Child(parent()):
    value = member()

assert events == ['parent', 'body']
assert Child.value == 1
");
}

#[test]
fn parent_failure_prevents_class_body_execution() {
    run(r"
events = []

def member():
    events.append('body')
    return 1

try:
    class Child(MissingParent):
        value = member()
except NameError:
    pass
else:
    assert False, 'an undefined parent must raise NameError'

assert events == []
");
}

#[test]
fn parent_argument_and_class_body_capture_use_distinct_slots() {
    run(r"
class Parent:
    pass

def outer(Parent):
    events = []
    def inner():
        class Child(Parent):
            Parent = events.append('body')
    inner()
    return events

assert outer(Parent) == ['body']
");
}

#[test]
fn single_parent_classes_can_be_constructed() {
    run(r"
class Parent:
    pass

class Child(Parent):
    value = 1

class Grandchild(Child):
    value = 2

assert Child.value == 1
assert Grandchild.value == 2
assert type(Child()) is Child
assert type(Grandchild()) is Grandchild

Dynamic = type('Dynamic', (Parent,), {'value': 3})
assert Dynamic.value == Dynamic().value == 3
assert type(Dynamic()) is Dynamic
");
}

#[test]
fn unsupported_bases_remain_rejected() {
    run(r"
class Parent:
    pass

for base in [42, int, Parent()]:
    try:
        type('Child', (base,), {})
    except TypeError:
        pass
    else:
        assert False, 'only sandbox-defined classes can be parents'

try:
    type('Child', (Parent, Parent), {})
except TypeError as exc:
    assert 'duplicate base class' in str(exc)
else:
    assert False, 'duplicate base class must be rejected'
");
}

#[test]
fn deeply_nested_subclass_checks_respect_recursion_limits() {
    let code = r"
class Parent: pass
class Child(Parent): pass
nested = Parent
for _ in range(100):
    nested = (nested,)
assert issubclass(Child, (Parent, nested))
try:
    issubclass(Child, nested)
except RecursionError:
    pass
else:
    assert False, 'nested classinfo must respect the recursion limit'
assert issubclass(Child, Parent)
";
    let tracker = ResourceTracker::new(ResourceLimits {
        max_recursion_depth: 16,
        ..ResourceLimits::default()
    });
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run(vec![], tracker, PrintWriter::Stdout)
        .unwrap();
}

#[test]
fn inherited_initialization_handles_defaults_overrides_and_mutation() {
    run(r"
class Parent:
    def __init__(self, value, *, offset=2):
        self.value = value + offset

class Child(Parent):
    pass

class Grandchild(Child):
    pass

assert Child(3).value == 5
assert Grandchild(3, offset=4).value == 7
Dynamic = type('Dynamic', (Grandchild,), {})
assert Dynamic(3).value == 5
assert type(Dynamic(3)) is Dynamic

class OwnInit(Parent):
    def __init__(self):
        self.value = 99

assert OwnInit().value == 99

def replacement(self, value=10):
    self.value = value * 2

Parent.__init__ = replacement
assert Child().value == 20
assert Grandchild(4).value == 8
assert OwnInit().value == 99
");
}

#[test]
fn inherited_initializer_errors_preserve_constructor_contracts() {
    run(r#"
class Parent:
    def __init__(self):
        raise ValueError('from parent')

class Child(Parent):
    pass

try:
    Child()
except ValueError as exc:
    assert str(exc) == 'from parent'
else:
    assert False, 'inherited initialization must propagate exceptions'

class Blocked(Parent):
    __init__ = None

try:
    Blocked()
except TypeError:
    pass
else:
    assert False, 'None must shadow the parent initializer'

def bad_return(self):
    return 42

Parent.__init__ = bad_return
try:
    Child()
except TypeError as exc:
    assert str(exc) == "__init__() should return None, not 'int'"
else:
    assert False, 'inherited initialization must return None'

class Empty:
    pass

class EmptyChild(Empty):
    pass

assert type(EmptyChild()) is EmptyChild
try:
    EmptyChild(1)
except TypeError:
    pass
else:
    assert False, 'classes without an initializer reject arguments'
"#);
}

#[test]
fn inherited_lookup_survives_dump_restore() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "class Parent:\n    value = 7\nclass Child(Parent): pass\ninstance = Child()\nSavedParent = Parent\nParent = None\ncheck_subclass = issubclass",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };
    let output = restored
        .feed_run(
            "assert isinstance(instance, SavedParent)\nassert check_subclass(Child, SavedParent)\ninstance.value + Child.value",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(output, MontyObject::int(14));
}

fn resume_inherited_initializer(round_trip: bool) {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl
        .feed_start(
            "class Parent:\n    def __init__(self):\n        self.value = ext_fn()\nclass Child(Parent): pass\ninstance = Child()\nassert type(instance) is Child\ninstance.value",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let progress = if round_trip {
        let bytes = dump("test.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
        let Session::Suspended(restored) = Dump::load(&bytes).unwrap().state else {
            panic!("expected a suspended initializer");
        };
        *restored
    } else {
        progress
    };
    let call = progress.into_function_call().expect("expected an external call");
    assert_eq!(call.function_name, "ext_fn");
    let completed = call.resume(MontyObject::int(41), PrintWriter::Stdout).unwrap();
    let (_, output) = completed.into_complete().expect("expected construction to complete");
    assert_eq!(output, MontyObject::int(41));
}

#[test]
fn inherited_initializer_can_suspend() {
    resume_inherited_initializer(false);
}

#[test]
#[cfg(not(feature = "memory-model-checks"))]
fn inherited_initializer_resumes_after_dump_restore() {
    resume_inherited_initializer(true);
}

#[test]
#[cfg(feature = "ref-count-return")]
fn parent_child_cycles_are_collected() {
    let code = r"
def make_cycle():
    class Parent:
        pass
    class Child(Parent):
        pass
    Parent.child = Child

for _ in range(10):
    make_cycle()
";
    let output = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_ref_counts(vec![])
        .unwrap();
    assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
}

#[test]
#[cfg(feature = "ref-count-return")]
fn rejected_class_namespaces_release_parent_references() {
    let code = r"
def exercise():
    class Parent: pass
    for namespace in [None, {1: 'invalid key'}]:
        try:
            type('Child', (Parent,), namespace)
        except TypeError:
            pass
for _ in range(10):
    exercise()
";
    let output = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_ref_counts(vec![])
        .unwrap();
    assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
}

#[test]
fn custom_new_controls_allocation_and_initialization() {
    run(r"
events = []
class Parent:
    def __new__(cls, value):
        events.append('new')
        result = object.__new__(cls)
        result.from_new = value
        return result
    def __init__(self, value):
        events.append('init')
        self.value = value

class Child(Parent):
    pass

child = Child(7)
assert type(child) is Child
assert child.from_new == child.value == 7
assert events == ['new', 'init']
assert child.__new__(Child, 8).__class__ is Child

class ReturnsNumber:
    def __new__(cls):
        return 42
    def __init__(self):
        assert False

assert ReturnsNumber() == 42

class NewOnly:
    def __new__(cls, value):
        result = object.__new__(cls)
        result.value = value
        return result

assert NewOnly(5).value == 5

class Other:
    pass

class ReturnsOther:
    def __new__(cls):
        return Other()
    def __init__(self):
        assert False

assert type(ReturnsOther()) is Other
");
}

#[test]
fn classes_and_bases_cannot_be_reassigned() {
    run(r"
class Parent:
    pass

class Child(Parent):
    pass

obj = Child()
for target in [Parent, Child]:
    for name in ['__class__', '__bases__']:
        try:
            setattr(target, name, Parent)
        except TypeError:
            pass
        else:
            assert False

try:
    setattr(obj, '__class__', Parent)
except TypeError:
    pass
else:
    assert False

obj.__bases__ = Parent
assert obj.__bases__ is Parent

try:
    object.__setattr__(obj, '__class__', Parent)
except TypeError:
    pass
else:
    assert False

assert obj.__class__ is Child
assert Parent.__bases__ == (object,)
assert Child.__class__ is type
assert Child.__bases__ == (Parent,)
assert isinstance(obj, Parent)
");
}

#[test]
fn native_exception_ancestry_and_identity_are_preserved() {
    run(r#"
class Error(ValueError):
    pass

class Child(Error):
    pass

error = Child('message', [1, 2])
assert type(error) is Child
assert error.args == ('message', [1, 2])
assert isinstance(error, ValueError)
assert isinstance(error, Exception)
assert isinstance(error, Error)
assert issubclass(Child, ValueError)
assert Child.__bases__ == (Error,)
assert Error.__bases__ == (ValueError,)
try:
    raise error
except Error as caught:
    assert caught is error
    try:
        raise
    except ValueError as reraised:
        assert reraised is error
else:
    assert False

try:
    raise Child
except Exception as caught:
    assert type(caught) is Child
else:
    assert False

class Custom(Exception):
    def __new__(cls, message):
        return Exception.__new__(cls, message)
    def __init__(self, message):
        self.message = message

custom = Custom('hello')
assert custom.args == ('hello',)
assert str(custom) == 'hello'
assert repr(custom) == "Custom('hello')"
try:
    raise custom
except Custom as caught:
    assert caught is custom
else:
    assert False

class Context:
    def __enter__(self):
        return self
    def __exit__(self, typ, value, traceback):
        assert typ is Child
        assert value is error
        return True

with Context():
    raise error

Dynamic = type('Dynamic', (Error,), {})
assert isinstance(Dynamic(), ValueError)
"#);
}

#[test]
fn object_is_the_root_of_custom_and_native_classes() {
    run(r"
class Parent:
    pass

class Child(Parent):
    pass

class Error(ValueError):
    pass

assert object.__bases__ == ()
assert Parent.__bases__ == (object,)
assert Child.__bases__ == (Parent,)
assert ValueError.__bases__ == (Exception,)
assert Exception.__bases__ == (BaseException,)
assert BaseException.__bases__ == (object,)
assert bool.__bases__ == (int,)
assert type.__bases__ == (object,)
import collections
Point = collections.namedtuple('Point', ['x'])
assert Point.__bases__ == (tuple,)
assert Point.__class__ is type
assert issubclass(Point, object)
assert isinstance(Point(1), object)

for cls in (Parent, Child, Error, ValueError, Exception, BaseException, bool, int, dict, tuple, str, type):
    assert issubclass(cls, object)
    assert cls.__class__ is type
for value in (Parent(), Child(), [], Error('x'), ValueError('x'), True, 1, {}, (), 'x', object):
    assert isinstance(value, object)
");
}

#[test]
fn super_zero_arg_and_cooperative_dispatch() {
    run(r#"
class Animal:
    def __init__(self, name):
        self.name = name

    def speak(self):
        return f"{self.name} makes a sound"

class Dog(Animal):
    def __init__(self, name, breed):
        super().__init__(name)
        self.breed = breed

    def speak(self):
        return f"{super().speak()} and barks"

d = Dog("Rex", "Collie")
assert d.name == "Rex"
assert d.breed == "Collie"
assert d.speak() == "Rex makes a sound and barks"

# Diamond multiple inheritance cooperative dispatch
class A:
    def greet(self):
        return ["A"]

class B(A):
    def greet(self):
        return ["B"] + super().greet()

class C(A):
    def greet(self):
        return ["C"] + super().greet()

class D(B, C):
    def greet(self):
        return ["D"] + super().greet()

d_obj = D()
assert d_obj.greet() == ["D", "B", "C", "A"]
"#);
}

#[test]
fn super_two_arg_and_introspection() {
    run(r#"
class Base:
    def foo(self):
        return "Base.foo"

class Mid(Base):
    def foo(self):
        return "Mid.foo"

class Sub(Mid):
    def foo(self):
        return "Sub.foo"
    def mid_foo(self):
        return super().foo()
    def base_foo(self):
        return super(Mid, self).foo()

s = Sub()
assert s.foo() == "Sub.foo"
assert s.mid_foo() == "Mid.foo"
assert s.base_foo() == "Base.foo"

# super(Type, SubType)
class Shape:
    kind = "generic shape"

class Polygon(Shape):
    kind = "polygon"

class Triangle(Polygon):
    kind = "triangle"

assert super(Triangle, Triangle).kind == "polygon"
assert super(Polygon, Triangle).kind == "generic shape"

# Introspection attributes
class X:
    pass

class Y(X):
    def check_super(self):
        s = super()
        assert s.__thisclass__ is Y
        assert s.__self__ is self
        assert s.__self_class__ is Y

y = Y()
y.check_super()

s_unbound = super(Y)
assert repr(s_unbound) == "<super: <class 'Y'>, NULL>"
assert s_unbound.__thisclass__ is Y
assert s_unbound.__self__ is None
assert s_unbound.__self_class__ is None

try:
    s_unbound.some_attr
except AttributeError as e:
    assert "'super' object has no attribute 'some_attr'" in str(e)
else:
    assert False

s_bound = super(Y, y)
assert repr(s_bound) == "<super: <class 'Y'>, <Y object>>"

s_class_bound = super(X, Y)
assert repr(s_class_bound) == "<super: <class 'X'>, <Y object>>"

# object.__init__ via super()
class Leaf:
    def __init__(self):
        super().__init__()

Leaf()
"#);
}

#[test]
fn super_errors() {
    run(r#"
class X:
    pass

class Y(X):
    pass

# Arg 1 not a type
try:
    super(1, 2)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "super() argument 1 must be a type, not int"

# Arg 2 not an instance or subtype
try:
    super(X, "not_an_instance")
    assert False, "expected TypeError"
except TypeError as e:
    assert "is not an instance or subtype of type" in str(e)

try:
    super(Y, X())
    assert False, "expected TypeError"
except TypeError as e:
    assert "is not an instance or subtype of type" in str(e)

# Kwargs rejected
try:
    super(foo=1)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "super() takes no keyword arguments"

# Too many arguments
try:
    super(X, Y(), 1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "super() expected at most 2 arguments, got 3" in str(e)

# Free function zero-arg super
def free_func():
    super()

try:
    free_func()
    assert False, "expected RuntimeError"
except RuntimeError as e:
    assert "super(): no arguments" in str(e)

# Zero-arg super outside any frame
try:
    eval("super()")
    assert False, "expected RuntimeError"
except RuntimeError as e:
    assert "super(): no current frame" in str(e) or "super(): no arguments" in str(e)
"#);
}

#[test]
fn init_subclass_hooks_and_class_kwargs() {
    run(r#"
class PluginBase:
    registry = []
    def __init_subclass__(cls, plugin_name="default", **kwargs):
        super().__init_subclass__(**kwargs)
        cls.plugin_name = plugin_name
        cls.registry.append(cls)

class PluginA(PluginBase, plugin_name="alpha"):
    pass

class PluginB(PluginBase, plugin_name="beta"):
    pass

assert PluginA.plugin_name == "alpha"
assert PluginB.plugin_name == "beta"
assert PluginBase.registry == [PluginA, PluginB]

# Base definition does not call its own __init_subclass__
calls = []
class Tracker:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        calls.append(cls.__name__)

assert calls == []

class SubTracker(Tracker):
    pass

assert calls == ["SubTracker"]

# Cooperative multi-level kwargs forwarding
log = []
class A:
    def __init_subclass__(cls, a=1, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(("A", cls.__name__, a))

class B(A):
    def __init_subclass__(cls, b=2, **kwargs):
        super().__init_subclass__(**kwargs)
        log.append(("B", cls.__name__, b))

assert log == [("A", "B", 1)]

class C(B, a=10, b=20):
    pass

assert log == [("A", "B", 1), ("A", "C", 10), ("B", "C", 20)]

# Dynamic type() call with kwargs
class DynBase:
    def __init_subclass__(cls, tag="", **kwargs):
        super().__init_subclass__(**kwargs)
        cls.tag = tag

DynSub = type("DynSub", (DynBase,), {}, tag="dynamic_plugin")
assert DynSub.tag == "dynamic_plugin"

# Direct __init_subclass__ call on builtin
assert int.__init_subclass__() is None
"#);
}

#[test]
fn init_subclass_and_kwargs_errors() {
    run(r#"
# Keyword arguments without __init_subclass__
try:
    class Standalone(extra="val"):
        pass
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "Standalone.__init_subclass__() takes no keyword arguments"

try:
    class ObjSub(object, extra="val"):
        pass
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "ObjSub.__init_subclass__() takes no keyword arguments"

# Direct invocation argument validation
class Foo:
    pass

try:
    Foo.__init_subclass__(1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "takes no arguments (1 given)" in str(e)

try:
    Foo.__init_subclass__(extra=1)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "Foo.__init_subclass__() takes no keyword arguments"

try:
    int.__init_subclass__(1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "takes no arguments (1 given)" in str(e)

try:
    int.__init_subclass__(extra=1)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "int.__init_subclass__() takes no keyword arguments"

# Failing __init_subclass__ aborts class creation
class FailingBase:
    def __init_subclass__(cls, **kwargs):
        raise ValueError("initialization failed")

try:
    class Child(FailingBase):
        pass
    assert False, "expected ValueError"
except ValueError as e:
    assert str(e) == "initialization failed"
"#);
}

#[test]
fn issubclass_builtin_and_type_checks() {
    run(r"
class Grandparent:
    pass

class Parent(Grandparent):
    pass

class Child(Parent):
    pass

class Unrelated:
    pass

assert issubclass(Child, Parent)
assert issubclass(Child, Grandparent)
assert issubclass(Child, object)
assert issubclass(Child, Child)
assert not issubclass(Parent, Child)
assert not issubclass(Unrelated, Parent)

# Builtin types
assert issubclass(bool, int)
assert issubclass(bool, object)
assert issubclass(int, object)
assert issubclass(str, object)
assert not issubclass(int, str)
assert not issubclass(str, int)

# Tuples as second arg
assert issubclass(Child, (int, Parent))
assert issubclass(Child, (Unrelated, (str, (float, Grandparent))))
assert not issubclass(Child, (int, str, Unrelated))

# Unions
assert issubclass(int, int | str)
assert issubclass(bool, str | int)
assert not issubclass(str, int | float)

# Type object itself
assert issubclass(type, object)
assert issubclass(type, type)
assert issubclass(Child, object)
assert not issubclass(Child, type)
");
}

#[test]
fn issubclass_errors() {
    run(r#"
class Base:
    pass

# Arg 1 not a class
try:
    issubclass(1, int)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "issubclass() arg 1 must be a class"

try:
    issubclass(Base(), Base)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "issubclass() arg 1 must be a class"

# Arg 2 not a class, tuple, or union
try:
    issubclass(Base, 1)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "issubclass() arg 2 must be a class, a tuple of classes, or a union"

try:
    issubclass(int, (str, 1))
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "issubclass() arg 2 must be a class, a tuple of classes, or a union"

# Generics rejected
try:
    issubclass(int, list[int])
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "issubclass() argument 2 cannot be a parameterized generic"

try:
    issubclass(list[int], list)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "issubclass() arg 1 must be a class"
"#);
}

#[test]
fn class_introspection_mro_and_bases() {
    run(r#"
class A:
    pass

class B(A):
    pass

class C(A):
    pass

class D(B, C):
    pass

assert A.__bases__ == (object,)
assert B.__bases__ == (A,)
assert D.__bases__ == (B, C)
assert object.__bases__ == ()
assert bool.__bases__ == (int,)
assert int.__bases__ == (object,)

assert D.__mro__ == (D, B, C, A, object)
assert D.mro() == [D, B, C, A, object]
assert bool.mro() == [bool, int, object]
assert object.mro() == [object]
assert int.mro() == [int, object]
assert type.mro() == [type, object]

# Calling mro with arguments raises TypeError
try:
    D.mro(1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "D.mro() takes no arguments (1 given)" in str(e)

try:
    bool.mro(1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "bool.mro() takes no arguments (1 given)" in str(e)

try:
    type.mro(1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "descriptor 'mro' for 'type' objects doesn't apply to a 'int' object" in str(e)

# Reassignment protection
try:
    D.__bases__ = ()
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "type controls __class__ and __bases__"

try:
    D.__class__ = object
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "type controls __class__ and __bases__"

# __eq__ without __hash__ sets __hash__ to None
class EqOnly:
    def __eq__(self, other):
        return True

assert EqOnly.__hash__ is None
try:
    hash(EqOnly())
    assert False, "expected TypeError"
except TypeError as e:
    assert "unhashable type: 'EqOnly'" in str(e)
"#);
}

#[test]
fn custom_exception_subclasses_and_matching() {
    run(r#"
class AppError(Exception):
    pass

class DatabaseError(AppError):
    def __init__(self, code, message):
        super().__init__()
        self.code = code
        self.message = message

class QueryError(DatabaseError):
    pass

assert issubclass(QueryError, DatabaseError)
assert issubclass(QueryError, AppError)
assert issubclass(QueryError, Exception)
assert issubclass(QueryError, BaseException)
assert QueryError.__mro__ == (QueryError, DatabaseError, AppError, Exception, BaseException, object)

# Raise and catch
caught = None
try:
    raise QueryError(1001, "table not found")
except AppError as e:
    caught = e
    assert isinstance(e, QueryError)
    assert isinstance(e, DatabaseError)
    assert isinstance(e, AppError)
    assert isinstance(e, Exception)
    assert e.code == 1001
    assert e.message == "table not found"

assert caught is not None

# ExcType.mro() and custom exception .mro()
assert ValueError.mro() == [ValueError, Exception, BaseException, object]
assert QueryError.mro() == [QueryError, DatabaseError, AppError, Exception, BaseException, object]

try:
    ValueError.mro(1)
    assert False, "expected TypeError"
except TypeError as e:
    assert "ValueError.mro() takes no arguments (1 given)" in str(e)

# Exception layout conflicts
try:
    class DuplicateErr(ValueError, ValueError):
        pass
    assert False, "expected TypeError"
except TypeError as e:
    assert "duplicate base class ValueError" in str(e)

# Sibling exceptions without custom layout combine cleanly
class SiblingErr(ValueError, TypeError):
    pass
assert issubclass(SiblingErr, ValueError)
assert issubclass(SiblingErr, TypeError)

try:
    class ConflictingErr(OSError, SyntaxError):
        pass
    assert False, "expected TypeError"
except TypeError as e:
    assert "multiple bases have instance lay-out conflict" in str(e)
"#);
}

#[test]
fn object_new_builtin_and_custom_new() {
    run(r#"
class Plain:
    def __init__(self):
        self.x = 10

p = object.__new__(Plain)
assert isinstance(p, Plain)
assert not hasattr(p, "x")

# Calling object.__new__ on native type
try:
    object.__new__(int)
    assert False, "expected TypeError"
except TypeError as e:
    assert "object.__new__(int) is not safe, use int.__new__()" in str(e)

# Calling object.__new__ on non-type
try:
    object.__new__(123)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "object.__new__(X): X is not a type object (int)"

# Calling object.__new__ without args
try:
    object.__new__()
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "object.__new__(): not enough arguments"

# Calling object.__new__ with extra arguments when __new__ is overridden
class PlainWithNew:
    def __new__(cls, *args, **kwargs):
        return super().__new__(cls)

try:
    object.__new__(PlainWithNew, 1)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "object.__new__() takes exactly one argument (the type to instantiate)"

try:
    object.__new__(PlainWithNew, x=1)
    assert False, "expected TypeError"
except TypeError as e:
    assert str(e) == "object.__new__() takes exactly one argument (the type to instantiate)"

# When __init__ is overridden without overriding __new__, extra args are accepted
p_extra = object.__new__(Plain, 1, x=2)
assert isinstance(p_extra, Plain)

# Calling __new__ on built-in exception types
v1 = ValueError.__new__(ValueError)
assert isinstance(v1, ValueError)
assert str(v1) == ""

v2 = ValueError.__new__(ValueError, "msg")
assert isinstance(v2, ValueError)
assert str(v2) == "msg"

# User class custom __new__ returning instance vs non-instance
class Singleton:
    _instance = None
    def __new__(cls, val):
        if cls._instance is None:
            cls._instance = object.__new__(cls)
            cls._instance.val = val
        return cls._instance

s1 = Singleton(1)
s2 = Singleton(2)
assert s1 is s2
assert s1.val == 1

class Factory:
    def __new__(cls):
        return 42

assert Factory() == 42
"#);
}

#[test]
fn inheritance_copy_module_and_snapshots() {
    run(r#"
import copy

class Base:
    def __init__(self, x):
        self.x = x

class Sub(Base):
    def __init__(self, x, y):
        super().__init__(x)
        self.y = y

s = Sub([1, 2], {"a": 1})
s_shallow = copy.copy(s)
assert s_shallow is not s
assert s_shallow.x is s.x
assert s_shallow.y is s.y

s_deep = copy.deepcopy(s)
assert s_deep is not s
assert s_deep.x is not s.x
assert s_deep.x == [1, 2]
assert s_deep.y is not s.y
assert s_deep.y == {"a": 1}
"#);

    // Repl session snapshotting and restoring class hierarchy with super()
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        r"
class A:
    def val(self):
        return 10

class B(A):
    def val(self):
        return super().val() + 20

b = B()
",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();

    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };

    let output = restored.feed_run("b.val()", vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(output, MontyObject::int(30));
}

#[test]
fn inheritance_edge_cases_and_error_paths() {
    run(r#"
# isinstance with type
class A: pass
assert isinstance(A, type)
assert isinstance(int, type)
assert isinstance(ValueError, type)
assert isinstance(type, type)
assert not isinstance(1, type)
assert not isinstance(A(), type)

# isinstance with Exception types
class CustomErr(ValueError): pass
class OtherErr(TypeError): pass
err = CustomErr("msg")
assert isinstance(err, CustomErr)
assert isinstance(err, ValueError)
assert isinstance(err, Exception)
assert isinstance(err, BaseException)
assert not isinstance(err, TypeError)
assert not isinstance(123, Exception)

# Exception hierarchy MROs exercising ExcType::parent()
assert KeyError.mro() == [KeyError, LookupError, Exception, BaseException, object]
assert IndexError.mro() == [IndexError, LookupError, Exception, BaseException, object]
assert FileNotFoundError.mro() == [FileNotFoundError, OSError, Exception, BaseException, object]
assert ZeroDivisionError.mro() == [ZeroDivisionError, ArithmeticError, Exception, BaseException, object]
assert KeyboardInterrupt.mro() == [KeyboardInterrupt, BaseException, object]
assert SystemExit.mro() == [SystemExit, BaseException, object]

# Custom exception inheriting from BaseException directly
class CustomBaseExc(BaseException): pass
class SubCustomBaseExc(CustomBaseExc): pass
assert SubCustomBaseExc.__mro__ == (SubCustomBaseExc, CustomBaseExc, BaseException, object)
assert issubclass(SubCustomBaseExc, BaseException)
assert not issubclass(SubCustomBaseExc, Exception)

# Multiple bases conflict and duplicate bases
try:
    class DupParent(A, A): pass
    assert False
except TypeError as e:
    assert "duplicate base class A" in str(e)

try:
    class ConflictExc(OSError, SyntaxError): pass
    assert False
except TypeError as e:
    assert "multiple bases have instance lay-out conflict" in str(e)

# Decorators combined with class kwargs
def tag(cls):
    cls.tagged = True
    return cls

class BasePlugin:
    def __init_subclass__(cls, k="default", **kwargs):
        super().__init_subclass__(**kwargs)
        cls.k = k

@tag
class ConcretePlugin(BasePlugin, k="custom"):
    pass

assert ConcretePlugin.tagged is True
assert ConcretePlugin.k == "custom"

# Unpack in bases with kwargs
bases = (BasePlugin,)
class UnpackSub(*bases, k="unpacked"):
    pass

assert UnpackSub.k == "unpacked"
"#);

    // Parse errors: metaclass and **kwargs unpacking
    assert!(
        MontyRun::new(
            "class WithMeta(metaclass=type): pass".to_owned(),
            "test.py",
            vec![],
            CompileOptions::default()
        )
        .is_err()
    );

    assert!(
        MontyRun::new(
            "kwargs = {}\nclass WithKwargsUnpack(**kwargs): pass".to_owned(),
            "test.py",
            vec![],
            CompileOptions::default()
        )
        .is_err()
    );
}

#[test]
fn nested_tuple_subclass_and_isinstance_checks_respect_time_limits() {
    let code_issubclass = r"
t = str
for _ in range(60):
    t = (t, t)
issubclass(int, t)
";
    let mut ex = MontyRun::new(code_issubclass.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(50));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err(), "should exceed time limit in issubclass");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);

    let code_isinstance = r"
t = str
for _ in range(60):
    t = (t, t)
isinstance(1, t)
";
    let mut ex = MontyRun::new(code_isinstance.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(50));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err(), "should exceed time limit in isinstance");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
}

#[test]
fn custom_exception_super_init_and_args() {
    run(r#"
class AppError(Exception):
    def __init__(self, message, code):
        super().__init__(message)
        self.code = code

err = AppError("bad request", 400)
assert err.args == ("bad request",)
assert err.code == 400

err2 = AppError(message="bad", code=400)
assert err2.args == ("bad",)
assert err2.code == 400

try:
    AppError("bad", 400, unknown=1)
except TypeError:
    pass
else:
    assert False

class EmptyError(Exception):
    pass

try:
    EmptyError(code=123)
except TypeError:
    pass
else:
    assert False
"#);
}

#[test]
fn custom_exception_formatting_and_identity() {
    let code = "class MyError(Exception): pass\nraise MyError('failed')";
    let res = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![]);
    assert!(res.is_err());
    let exc = res.unwrap_err();
    assert_eq!(exc.custom_type_name(), Some("MyError"));
    assert_eq!(exc.message(), Some("failed"));
    assert!(exc.to_string().contains("MyError: failed"));

    let code_none = "class MyError(Exception): pass\nraise MyError()";
    let res_none = MontyRun::new(code_none.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![]);
    assert!(res_none.is_err());
    let exc_none = res_none.unwrap_err();
    assert_eq!(exc_none.custom_type_name(), Some("MyError"));
    assert_eq!(exc_none.message(), None);
    assert!(exc_none.to_string().ends_with("MyError"));
}

#[test]
fn custom_exception_formatting_terminal_error_propagates() {
    let code = r"
class InfiniteFormatError(Exception):
    def __str__(self):
        while True:
            pass

raise InfiniteFormatError()
";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(50));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err());
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
}

#[test]
fn custom_exception_repr_cycle_detection() {
    run(r#"
class MyError(Exception):
    pass

e = MyError()
e.args = (e,)
assert repr(e) == "MyError(...)"
"#);
}

#[test]
fn super_new_and_unbound_super() {
    run(r#"
class A:
    def __new__(cls):
        return super().__new__(cls)

a = A()
assert isinstance(a, A)

class Parent:
    def __new__(cls, val):
        obj = super().__new__(cls)
        obj.val = val
        return obj

class Child(Parent):
    def __new__(cls, val):
        return super().__new__(cls, val * 2)

c = Child(5)
assert c.val == 10

s = super(Child, None)
assert repr(s) == "<super: <class 'Child'>, NULL>"
assert s.__self__ is None
assert s.__self_class__ is None
assert s.__thisclass__ is Child

class Root:
    def f(self):
        return 42

class Base(Root):
    pass

class Sub(Base):
    pass

unbound = super(Base, Sub).f
sub_obj = Sub()
assert unbound(sub_obj) == 42
"#);
}

#[test]
fn inconsistent_mro_and_exception_bases_order() {
    run(r"
class MyValErr(ValueError):
    pass

try:
    class Bad(ValueError, MyValErr):
        pass
except TypeError:
    pass
else:
    assert False

class Mixin:
    pass

class Combined(ValueError, Mixin):
    pass

assert Combined.__bases__ == (ValueError, Mixin)
assert Combined.__mro__[0] is Combined
assert Combined.__mro__[1] is ValueError
assert Combined.__mro__[-2] is Mixin
assert Combined.__mro__[-1] is object
");
}

#[test]
fn instance_bases_reassignment_and_super_cell_spoofing() {
    run(r"
class Foo:
    pass

f = Foo()
f.__bases__ = 42
assert f.__bases__ == 42

class MethodSpoof:
    def f(self):
        __class__ = 42
        return super()

try:
    MethodSpoof().f()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'super(): __class__ cell not found'

def make_outer_spoof():
    __class__ = 42
    class OuterSpoof:
        def f(self):
            return super()
    return OuterSpoof

OuterSpoof = make_outer_spoof()
s = OuterSpoof().f()
assert s.__thisclass__ is OuterSpoof
");
}

#[test]
fn custom_exception_caught_across_evaluate_function_boundary() {
    run(r"
class MyError(Exception):
    pass

def bad_key(x):
    raise MyError('from key')

caught = False
try:
    sorted([1, 2], key=bad_key)
except MyError as e:
    caught = True
    assert isinstance(e, MyError)

assert caught
");
}

#[test]
fn method_aliasing_and_self_reassignment_in_super() {
    run(r"
class Base:
    def m(self):
        return 'base'

class Mid(Base):
    def m(self):
        return super().m()

class Sub(Mid):
    alias = Mid.m

assert Sub().m() == 'base'

class ReassignSelf(Base):
    def m(self):
        self = Base()
        super()

try:
    ReassignSelf().m()
except TypeError as e:
    assert 'is not an instance or subtype' in str(e)
else:
    assert False
");
}

#[test]
fn super_init_mro_position_with_mixin_and_base_exception() {
    run(r"
class Mixin:
    def __init__(self):
        self.mixin_init_called = True

class Combined(Exception, Mixin):
    def __init__(self, msg):
        super().__init__(msg)

c = Combined('hello')
assert c.args == ('hello',)
assert not hasattr(c, 'mixin_init_called')

class MyError(Exception):
    pass

err = MyError('msg')
try:
    super(BaseException, err).__init__(1, 2)
except TypeError as e:
    assert 'object.__init__() takes exactly one argument' in str(e)
else:
    assert False
");
}

#[test]
fn sibling_exception_multiple_inheritance_and_layout_conflicts() {
    run(r#"
# Standard exception siblings without custom layouts combine cleanly
class E1(KeyError, AttributeError):
    pass

e1 = E1("msg")
assert isinstance(e1, KeyError)
assert isinstance(e1, AttributeError)
assert isinstance(e1, Exception)
assert E1.__mro__ == (E1, KeyError, LookupError, AttributeError, Exception, BaseException, object)

class E2(ValueError, TypeError):
    pass

e2 = E2("val and type")
assert isinstance(e2, ValueError)
assert isinstance(e2, TypeError)

# Standard exception and custom layout exception (OSError) combine cleanly
class E3(KeyError, OSError):
    pass

e3 = E3("key and os")
assert isinstance(e3, KeyError)
assert isinstance(e3, OSError)

# Multiple conflicting custom layout exception bases raise layout conflict
try:
    class Bad1(OSError, SyntaxError):
        pass
except TypeError as e:
    assert "multiple bases have instance lay-out conflict" in str(e)
else:
    assert False

try:
    class Bad2(OSError, SystemExit):
        pass
except TypeError as e:
    assert "multiple bases have instance lay-out conflict" in str(e)
else:
    assert False
"#);
}

#[test]
fn class_keyword_arguments_evaluated_before_class_body() {
    run(r#"
log = []

def track(x):
    log.append(x)
    return x

def fail():
    track("fail")
    raise ValueError("boom")

class Base:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__()
        cls.saved_kwargs = kwargs

class C1(Base, k=track("kw")):
    _ = track("body")

assert log == ["kw", "body"]
assert C1.saved_kwargs == {"k": "kw"}

class C2(Base, a=track("kw_a"), b=track("kw_b")):
    _ = track("body_2")

assert log == ["kw", "body", "kw_a", "kw_b", "body_2"]
assert C2.saved_kwargs == {"a": "kw_a", "b": "kw_b"}

try:
    class C3(Base, k=fail()):
        _ = track("should_not_run")
except ValueError as e:
    assert str(e) == "boom"
else:
    assert False

assert "should_not_run" not in log
"#);
}

#[test]
fn custom_new_subclass_init_and_dataclass_and_exception_args() {
    run(r#"
# 1. __new__ returning subclass instance runs subclass __init__
class Base:
    def __new__(cls, which):
        if which == "sub":
            return object.__new__(Sub)
        return object.__new__(cls)

class Sub(Base):
    def __init__(self, which):
        self.ran_sub_init = True

obj = Base("sub")
assert isinstance(obj, Sub)
assert getattr(obj, "ran_sub_init", False) is True

# 2. Dataclass with __new__ runs dataclass field binding
from dataclasses import dataclass

@dataclass
class Point:
    x: int
    y: int
    def __new__(cls, x, y):
        return object.__new__(cls)

p = Point(10, 20)
assert p.x == 10
assert p.y == 20

# 3. Custom exception __new__ sets args and rejects kwargs
class CustomExc(ValueError):
    def __new__(cls, *args, **kwargs):
        return super().__new__(cls)

e = CustomExc("hello", 42)
assert e.args == ("hello", 42)
assert isinstance(e, ValueError)

try:
    CustomExc("hello", bad_kw=1)
except TypeError as err:
    assert "takes no keyword arguments" in str(err)
else:
    assert False
"#);
}

#[test]
fn namedtuple_mro_and_class_custom_mro_and_init_subclass() {
    run(r#"
from collections import namedtuple

Point = namedtuple("Point", ["x", "y"])
assert Point.__mro__ == (Point, tuple, object)
assert Point.mro() == [Point, tuple, object]

# User-defined mro() is not shadowed
class CustomMro:
    def mro():
        return "custom_mro"

assert CustomMro.mro() == "custom_mro"

# Foo.__init_subclass__() direct call receives cls
class HookBase:
    captured = None
    def __init_subclass__(cls):
        HookBase.captured = cls

HookBase.__init_subclass__()
assert HookBase.captured is HookBase
"#);
}

#[test]
fn exception_types_name_bases_mro_and_subtype_check() {
    assert_eq!(
        ExcType::UnsupportedOperation.direct_bases(),
        &[ExcType::OSError, ExcType::ValueError]
    );
    assert_eq!(
        ExcType::UnsupportedOperation.mro(),
        vec![
            ExcType::UnsupportedOperation,
            ExcType::OSError,
            ExcType::ValueError,
            ExcType::Exception,
            ExcType::BaseException,
        ]
    );

    run(r#"
import json

assert json.JSONDecodeError.__name__ == "JSONDecodeError"
assert json.JSONDecodeError.__bases__ == (ValueError,)
assert json.JSONDecodeError.__mro__ == (json.JSONDecodeError, ValueError, Exception, BaseException, object)
assert json.JSONDecodeError.mro() == [json.JSONDecodeError, ValueError, Exception, BaseException, object]

try:
    ValueError.__new__(TypeError)
except TypeError as e:
    assert "ValueError.__new__(TypeError): TypeError is not a subtype of ValueError" in str(e)
else:
    assert False

class MyVal(ValueError):
    pass

inst = ValueError.__new__(MyVal)
assert type(inst) is MyVal
"#);
}

#[test]
fn isinstance_tuple_with_type_and_exception() {
    run(r"
assert isinstance(int, (type,))
assert isinstance(int, (int, (type,)))
assert isinstance(int, int | type)

class CustomErr(Exception):
    pass

err = CustomErr()
assert isinstance(err, (Exception,))
assert isinstance(err, (ValueError, (Exception,)))
assert isinstance(err, ValueError | Exception)
");
}

#[test]
fn object_new_builtin_safe_errors() {
    run(r#"
try:
    object.__new__(int)
except TypeError as e:
    assert "object.__new__(int) is not safe, use int.__new__()" in str(e)
else:
    assert False

try:
    object.__new__(type)
except TypeError as e:
    assert "object.__new__(type) is not safe, use type.__new__()" in str(e)
else:
    assert False
"#);
}

#[test]
fn datetime_and_collections_bases_and_mro() {
    run(r"
from datetime import datetime, date
from collections import defaultdict, Counter

assert datetime.__bases__ == (date,)
assert datetime.__mro__ == (datetime, date, object)
assert datetime.mro() == [datetime, date, object]
assert issubclass(datetime, date)

assert defaultdict.__bases__ == (dict,)
assert defaultdict.__mro__ == (defaultdict, dict, object)
assert defaultdict.mro() == [defaultdict, dict, object]
assert issubclass(defaultdict, dict)

assert Counter.__bases__ == (dict,)
assert Counter.__mro__ == (Counter, dict, object)
assert Counter.mro() == [Counter, dict, object]
assert issubclass(Counter, dict)
");
}

#[test]
fn bare_raise_traceback_location() {
    run_err(
        r"
class CustomErr(Exception):
    def __init__(self):
        pass

def f():
    raise CustomErr

f()
",
        "File \"test.py\", line 7, in f\n    raise CustomErr\nCustomErr",
    );
}

#[test]
fn builtins_type_and_exception_introspection_coverage() {
    run(r"
assert ValueError.__name__ == 'ValueError'
assert ValueError.__class__ == type
assert ValueError.__bases__ == (Exception,)
assert BaseException.__bases__ == (object,)
assert ValueError.__mro__ == (ValueError, Exception, BaseException, object)
assert ValueError.__new__ is not None

assert type.__name__ == 'type'
assert type.__class__ == type
assert type.__bases__ == (object,)
assert type.__mro__ == (type, object)
assert type.mro() == [type, object]

assert type(type) == type
assert type(ValueError) == type

from collections import namedtuple
Point = namedtuple('Point', ['x', 'y'])
assert Point.__mro__ == (Point, tuple, object)
assert Point.__class__ == type

assert issubclass(type, type)
assert not issubclass(ValueError, int)
assert not issubclass(int, tuple)

assert issubclass(RecursionError, RuntimeError)
assert issubclass(NotImplementedError, RuntimeError)
assert issubclass(FrozenInstanceError, AttributeError)
assert issubclass(UnboundLocalError, NameError)
assert issubclass(ModuleNotFoundError, ImportError)
assert issubclass(TimeoutError, OSError)
assert issubclass(PermissionError, OSError)

class MyCustom: pass
assert not issubclass(int, MyCustom)
assert not issubclass(MyCustom, int)
");

    run_err(
        "ValueError.nonexistent_attr",
        "type object 'ValueError' has no attribute 'nonexistent_attr'",
    );
    run_err(
        "type.nonexistent_attr",
        "type object 'type' has no attribute 'nonexistent_attr'",
    );
    run_err(
        "type.mro(1)",
        "descriptor 'mro' for 'type' objects doesn't apply to a 'int' object",
    );
    run_err(
        "ValueError.__new__(TypeError)",
        "ValueError.__new__(TypeError): TypeError is not a subtype of ValueError",
    );
    run_err("ValueError.__new__()", "ValueError.__new__(): not enough arguments");
    run_err(
        "from collections import namedtuple\nPoint = namedtuple('Point', ['x', 'y'])\nPoint.nonexistent_attr",
        "type object 'Point' has no attribute 'nonexistent_attr'",
    );
    run_err(
        "issubclass(int, 2)",
        "issubclass() arg 2 must be a class, a tuple of classes, or a union",
    );
}

#[test]
fn custom_new_and_args_coverage() {
    run(r"
class C:
    def __new__(cls, *args, **kwargs):
        inst = object.__new__(cls)
        inst.args = args
        inst.kwargs = kwargs
        return inst

c1 = C(1, 2)
assert c1.args == (1, 2)
c2 = C(x=10)
assert c2.kwargs == {'x': 10}
c3 = C(1, x=20)
assert c3.args == (1,) and c3.kwargs == {'x': 20}
c4 = C(**{'a': 1, 'b': 2})
assert c4.kwargs == {'a': 1, 'b': 2}

from dataclasses import dataclass
@dataclass
class D:
    val: int
    def __new__(cls, *args, **kwargs):
        return object.__new__(cls)

d = D(42)
assert d.val == 42

class E(Exception):
    def __new__(cls, *args, **kwargs):
        return super().__new__(cls)

e = E('hello', 100)
assert e.args == ('hello', 100)
");

    run_err(
        r"
class E(Exception):
    def __new__(cls, *args, **kwargs):
        return object.__new__(cls)
E()
",
        "object.__new__(E) is not safe, use E.__new__()",
    );

    run_err(
        r"
class E2(Exception):
    def __new__(cls, *args, **kwargs):
        dyn = type('DynExc', (cls,), {})
        return object.__new__(dyn)
E2()
",
        "object.__new__(DynExc) is not safe, use DynExc.__new__()",
    );

    run_err(
        "object.__new__(int)",
        "object.__new__(int) is not safe, use int.__new__()",
    );
    run_err(
        "object.__new__(type)",
        "object.__new__(type) is not safe, use type.__new__()",
    );
    run_err("class A: pass\nobject.__new__(A, 1, 2)", "A() takes no arguments");
}

#[test]
fn super_edge_cases_coverage() {
    run(r"
class A:
    def f(self): return 'A'

class B(A):
    def f(self): return super().f()

b = B()
assert b.f() == 'A'
s = super(B, b)
assert repr(s).startswith('<super: <class \'B\'>, <B object>>')
assert not (s == 123)
assert hash(s) == hash(s)
assert type(s) is super
assert type(b.f).__name__ == 'function'

class Base: pass
class Derived(Base): pass
s_cls = super(Base, Derived)
assert repr(s_cls).startswith('<super: <class \'Base\'>, <Derived object>>')

try:
    for x in s: pass
    assert False, 'iterating super should raise TypeError'
except TypeError:
    pass

try:
    for x in ValueError(): pass
    assert False, 'iterating exception should raise TypeError'
except TypeError:
    pass

class CycleExc(Exception): pass
c_exc = CycleExc()
c_exc.args = (c_exc,)
assert repr(c_exc) == 'CycleExc(...)'
");

    run_err(
        "class CycleExc(Exception): pass\nc = CycleExc()\nc.args = 123",
        "'int' object is not iterable",
    );

    run_err(
        "class A: pass\nclass B: pass\nsuper(A, B)",
        "super(type, obj): obj (type B) is not an instance or subtype of type (A).",
    );
    run_err(
        "class A: pass\nclass B: pass\nsuper(A, B())",
        "super(type, obj): obj (instance of B) is not an instance or subtype of type (A).",
    );
    run_err("class A: pass\nlen(super(A, A()))", "'super' has no len()");
    run_err(
        r"
class MyExc(Exception):
    def __init__(self, **kwargs):
        super().__init__(**kwargs)
MyExc(x=1)
",
        "BaseException.__init__() takes no keyword arguments",
    );
    run_err(
        r"
class A:
    def __init__(self, x):
        super().__init__(x)
A(1)
",
        "object.__init__() takes exactly one argument",
    );
    run_err(
        r"
class A:
    def f(self):
        super().nonexistent()
A().f()
",
        "'super' object has no attribute 'nonexistent'",
    );
}

#[test]
fn type_new_conflicts_and_exception_matching_coverage() {
    run_err(
        r"
class E1(SyntaxError): pass
class E2(OSError): pass
type('Conflict', (E1, E2), {})
",
        "multiple bases have instance lay-out conflict",
    );
    run(r"
from json import JSONDecodeError
c1 = type('C1', (SyntaxError, KeyboardInterrupt), {})
assert issubclass(c1, SyntaxError) and issubclass(c1, KeyboardInterrupt)
c3 = type('C3', (SyntaxError, JSONDecodeError), {})
assert issubclass(c3, SyntaxError) and issubclass(c3, JSONDecodeError)
");
    run_err(
        "type('C2', (SyntaxError, UnicodeDecodeError), {})",
        "multiple bases have instance lay-out conflict",
    );
    run_err(
        "type('C4', (SyntaxError, OSError), {})",
        "multiple bases have instance lay-out conflict",
    );

    run(r"
class CustomExc(Exception): pass

handled = False
try:
    raise ValueError('test')
except CustomExc:
    assert False
except ValueError:
    handled = True
assert handled
");
}

#[test]
fn class_keyword_walrus_and_base_unpacking_coverage() {
    run(r"
class BaseHook:
    def __init_subclass__(cls, **kwargs):
        cls.saved_kwargs = kwargs

# Walrus in class keyword argument
class MyClass(BaseHook, k=(x := 10)):
    pass

assert x == 10
assert MyClass.saved_kwargs == {'k': 10}

# Class capturing outer local in keyword
def outer():
    y = 20
    class Inner(BaseHook, k=y):
        pass
    return Inner

assert outer().saved_kwargs == {'k': 20}

# Base class unpacking
bases = (object,)
class UnpackedBase(*bases):
    pass

assert issubclass(UnpackedBase, object)
");
}

#[test]
fn class_creation_method_deduplication_and_time_limits() {
    run(r"
def f(self):
    return 1

def g(self):
    return super(Child, self).m1() + 2

def free_zero_super(self):
    return super().m1()

# Base has same function bound to multiple names
Base = type('Base', (), {'m1': f, 'm2': f, 'm3': f})
Child = type('Child', (Base,), {'m1': g, 'm2': g, 'bad': free_zero_super})

c = Child()
assert c.m1() == 3
assert c.m2() == 3

try:
    c.bad()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'super(): __class__ cell not found'
");

    let code = r"
def f(self): pass
ns = {str(i): f for i in range(10_000)}
Base = type('Base', (), ns)
";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_nanos(1));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err(), "should exceed time limit in type.__new__");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
}

#[test]
fn custom_exception_caught_does_not_execute_str_or_leak_unwinding_exception() {
    run(r"
str_calls = 0

class CustomErr(Exception):
    def __str__(self):
        global str_calls
        str_calls += 1
        return 'formatted'

class Trigger:
    def __repr__(self):
        raise CustomErr()

try:
    repr(Trigger())
except CustomErr:
    pass

assert str_calls == 0
");
}

#[test]
fn custom_exception_unhandled_str_memory_or_timeout_propagates_fatal() {
    let code = r"
class MemEscape(Exception):
    def __str__(self):
        _ = [0] * 100_000_000
        return 'boom'

raise MemEscape()
";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_memory(1024 * 1024);
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err());
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);

    let code_timeout = r"
class TimeoutEscape(Exception):
    def __str__(self):
        while True:
            pass

raise TimeoutEscape()
";
    let mut ex = MontyRun::new(code_timeout.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_nanos(1));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err());
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
}

#[test]
fn multi_base_builtin_exception_mro_and_subtyping() {
    run(r"
import decimal

class CustomDivZero(decimal.DivisionByZero):
    pass

assert issubclass(CustomDivZero, decimal.DivisionByZero)
assert issubclass(CustomDivZero, decimal.DecimalException)
assert issubclass(CustomDivZero, ZeroDivisionError)
assert issubclass(CustomDivZero, ArithmeticError)
assert issubclass(CustomDivZero, Exception)

try:
    raise CustomDivZero('division')
except ZeroDivisionError as err:
    assert str(err) == 'division'
except Exception:
    assert False, 'should have been caught by ZeroDivisionError'
else:
    assert False, 'should have raised'
");
}

#[test]
fn multiple_builtin_exception_bases_instantiation_and_new() {
    run(r"
class DualErr(KeyError, AttributeError):
    pass

assert issubclass(DualErr, KeyError)
assert issubclass(DualErr, AttributeError)

# Direct instantiation
err = DualErr('test')
assert isinstance(err, KeyError)
assert isinstance(err, AttributeError)

# __new__ dispatch through either base
err_attr = AttributeError.__new__(DualErr, 'attr')
assert isinstance(err_attr, DualErr)
assert isinstance(err_attr, AttributeError)
assert isinstance(err_attr, KeyError)

err_key = KeyError.__new__(DualErr, 'key')
assert isinstance(err_key, DualErr)
assert isinstance(err_key, KeyError)
assert isinstance(err_key, AttributeError)
");
}

#[test]
fn custom_key_error_formats_with_single_quote_repr() {
    run(r#"
class CustomKeyErr(ValueError, KeyError):
    pass

err = CustomKeyErr('my_missing_key')
assert str(err) == "'my_missing_key'"
"#);
}

#[test]
fn exception_args_coerced_to_tuple() {
    run(r"
class MyErr(Exception):
    pass

err = MyErr(1, 2)
assert err.args == (1, 2)
assert type(err.args) is tuple

err.args = [3, 4]
assert err.args == (3, 4)
assert type(err.args) is tuple

err.args = (x for x in [5, 6])
assert err.args == (5, 6)
assert type(err.args) is tuple

object.__setattr__(err, 'args', [7, 8])
assert err.args == (7, 8)
assert type(err.args) is tuple

try:
    err.args = 123
except TypeError as e:
    assert 'iterable' in str(e)
else:
    assert False, 'expected TypeError'

try:
    object.__setattr__(err, 'args', 123)
except TypeError as e:
    assert 'iterable' in str(e)
else:
    assert False, 'expected TypeError'
");
}

#[test]
fn custom_exception_has_custom_type_name_and_summary() {
    let code = r"
class SpecialFailure(ValueError):
    pass

raise SpecialFailure('bad value')
";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let err = ex.run_no_limits(vec![]).unwrap_err();
    assert_eq!(err.exc_type(), ExcType::ValueError);
    assert_eq!(err.custom_type_name(), Some("SpecialFailure"));
    assert_eq!(err.summary(), "SpecialFailure: bad value");
}

#[test]
fn descriptor_parity_and_hasattr() {
    run(r"
class A:
    pass

class B(A):
    def test_super(self):
        s = super()
        assert hasattr(s, '__init__')
        assert hasattr(s, '__init_subclass__')
        assert hasattr(s, '__new__')
        assert s.__init__() is None
        assert s.__init_subclass__() is None

        # object.__init__ extra args error
        try:
            s.__init__(1, 2)
        except TypeError as e:
            assert 'takes exactly one argument' in str(e)
        else:
            assert False, 'expected TypeError'

b = B()
b.test_super()

# Class descriptors
assert hasattr(A, 'mro')
assert hasattr(A, '__init_subclass__')
assert hasattr(A, '__new__')
assert hasattr(A, '__init__')

assert A.mro() == [A, object]
assert type.mro(A) == [A, object]
assert A.__init_subclass__() is None

a = A.__new__(A)
assert isinstance(a, A)
assert A.__init__(a) is None

# Instance descriptors
inst = A()
assert hasattr(inst, '__init__')
assert hasattr(inst, '__init_subclass__')
assert hasattr(inst, '__new__')
assert not hasattr(inst, 'mro')

assert inst.__init__() is None
assert inst.__init_subclass__() is None
a2 = inst.__new__(A)
assert isinstance(a2, A)

# Subclass hook binding
hook_called = []
class HookBase:
    def __init_subclass__(cls, **kwargs):
        hook_called.append((cls, kwargs))

assert hasattr(HookBase, '__init_subclass__')
h = HookBase()
assert hasattr(h, '__init_subclass__')
h.__init_subclass__(x=42)
assert hook_called == [(HookBase, {'x': 42})]

# Inherited __init_subclass__ called directly and via getattr on subclass
class HookChild(HookBase):
    pass

hook_called.clear()
HookChild.__init_subclass__(y=99)
fn_hook = HookChild.__init_subclass__
fn_hook(z=100)
assert hook_called == [(HookChild, {'y': 99}), (HookChild, {'z': 100})]

# Custom mro method on user class takes precedence over type.mro on class/instance
class CustomMro:
    def mro(self):
        return 'instance_mro'

assert CustomMro().mro() == 'instance_mro'
assert CustomMro.mro(CustomMro()) == 'instance_mro'
assert type.mro(CustomMro) == [CustomMro, object]

# Exception subclass __new__ and __init__ descriptor parity
class SubExc(ValueError):
    pass

fn_ve_new = ValueError.__new__
e1 = fn_ve_new(SubExc, 'a', 1)
assert isinstance(e1, SubExc) and e1.args == ('a', 1)

fn_sub_new = SubExc.__new__
e2 = fn_sub_new(SubExc, 'b')
assert isinstance(e2, SubExc) and e2.args == ('b',)

e3 = SubExc.__new__(SubExc, 'c')
assert isinstance(e3, SubExc) and e3.args == ('c',)
e4 = e3.__new__(SubExc, 'd')
assert isinstance(e4, SubExc) and e4.args == ('d',)

fn_init = e4.__init__
fn_init('updated', 2)
assert e4.args == ('updated', 2)

class SubExcSuper(SubExc):
    def __init__(self, msg):
        fn = super().__init__
        fn(msg, 'via_super')

    def make(cls):
        fn = super().__new__
        return fn(cls, 'made')

e5 = SubExcSuper('hi')
assert e5.args == ('hi', 'via_super')
e6 = SubExcSuper.make(SubExcSuper)
assert isinstance(e6, SubExcSuper) and e6.args == ('made',)
");
}

#[test]
fn dynamic_annotations_isolation() {
    run(r"
class AnnBase:
    x: int
    y: str

assert AnnBase.__annotations__ == {'x': int, 'y': str}

DynSub = type('DynSub', (AnnBase,), {})
a1 = DynSub.__annotations__
a2 = DynSub.__annotations__
assert a1 == {}
assert a1 is a2
a1['z'] = 'bool'
assert DynSub.__annotations__ == {'z': 'bool'}
# Confirm base annotations are untouched
assert AnnBase.__annotations__ == {'x': int, 'y': str}
");
}

#[test]
fn class_mro_write_protection() {
    run(r"
class A:
    pass

try:
    A.__mro__ = ()
except AttributeError as e:
    assert 'attribute \'__mro__\' of \'type\' objects is not writable' in str(e)
else:
    assert False, 'expected AttributeError'
");
}

#[test]
fn object_new_safety_and_excess_args() {
    run(r"
# object.__new__(object)
try:
    object.__new__(object)
except TypeError as e:
    assert 'cannot create \'object\' instances' in str(e)
else:
    assert False, 'expected TypeError'

# Builtin exception rejection
try:
    object.__new__(ValueError)
except TypeError as e:
    assert 'object.__new__(ValueError) is not safe, use ValueError.__new__()' in str(e)
else:
    assert False, 'expected TypeError'

# Custom exception rejection
class MyCustomExc(Exception):
    pass

try:
    object.__new__(MyCustomExc)
except TypeError as e:
    assert 'object.__new__(MyCustomExc) is not safe, use MyCustomExc.__new__()' in str(e)
else:
    assert False, 'expected TypeError'

# Excess args when neither __new__ nor __init__ is overridden
class Bare:
    pass

try:
    object.__new__(Bare, 1, 2)
except TypeError as e:
    assert 'Bare() takes no arguments' in str(e)
else:
    assert False, 'expected TypeError'

# Excess args when __new__ is overridden
class CustomNew:
    def __new__(cls, *args, **kwargs):
        return super().__new__(cls)

try:
    object.__new__(CustomNew, 1)
except TypeError as e:
    assert 'object.__new__() takes exactly one argument' in str(e)
else:
    assert False, 'expected TypeError'

# Excess args when __init__ is overridden (allowed!)
class CustomInit:
    def __init__(self, x, y=10):
        self.x = x
        self.y = y

inst = object.__new__(CustomInit, 1, y=20)
assert isinstance(inst, CustomInit)
assert not hasattr(inst, 'x')  # __init__ hasn't run yet
inst.__init__(1, y=20)
assert inst.x == 1
assert inst.y == 20
");
}

#[test]
fn dataclass_frozen_inheritance_mismatch() {
    run(r"
from dataclasses import dataclass

@dataclass(frozen=True)
class FrozenBase:
    x: int

try:
    @dataclass(frozen=False)
    class NonFrozenSub(FrozenBase):
        y: int
except TypeError as e:
    assert 'cannot inherit non-frozen dataclass from a frozen one' in str(e)
else:
    assert False, 'expected TypeError'

@dataclass(frozen=False)
class NonFrozenBase:
    x: int

try:
    @dataclass(frozen=True)
    class FrozenSub(NonFrozenBase):
        y: int
except TypeError as e:
    assert 'cannot inherit frozen dataclass from a non-frozen one' in str(e)
else:
    assert False, 'expected TypeError'

# Multi-level inheritance through intermediate plain class
class Intermediate(FrozenBase):
    pass

try:
    @dataclass(frozen=False)
    class Child(Intermediate):
        z: int
except TypeError as e:
    assert 'cannot inherit non-frozen dataclass from a frozen one' in str(e)
else:
    assert False, 'expected TypeError'
");
}

#[test]
fn super_class_cell_and_subclass_ops_survive_dump_restore() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        r"
class Base:
    def val(self):
        return 10
    def __add__(self, other):
        return 'base_add'
    def __eq__(self, other):
        return 'base_eq'

def add_one(cls):
    class Wrapped(cls):
        def val(self):
            return (lambda s: super().val() + 1)(self)
        def cls_obj(self):
            return (lambda: __class__)()
    return Wrapped

C = add_one(add_one(Base))

class Sub(Base):
    def __radd__(self, other):
        return 'sub_radd'
    def __eq__(self, other):
        return 'sub_eq'
",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };
    let output = restored
        .feed_run(
            "assert C().cls_obj() is C\nassert Base() + Sub() == 'sub_radd'\nassert (Base() == Sub()) == 'sub_eq'\nC().val()",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(output, MontyObject::int(12));
}

#[test]
fn host_class_type_inheritance_issubclass_super_and_new() {
    let code = r#"
class Plain:
    pass

assert issubclass(host_cls, host_cls)
assert not issubclass(host_cls, other_cls)
assert issubclass(host_cls, object)
assert not issubclass(host_cls, int)
assert not issubclass(host_cls, Plain)
assert not issubclass(Plain, host_cls)
assert not issubclass(int, host_cls)
assert isinstance(host_cls, type)
assert isinstance(host_inst, host_cls)
assert not isinstance(host_inst, other_cls)

s_inst = super(host_cls, host_inst)
assert repr(s_inst) == "<super: <class 'HostPt'>, <HostPt object>>"
assert s_inst.__self_class__ is host_cls

s_cls = super(object, host_cls)
assert repr(s_cls) == "<super: <class 'object'>, <HostPt object>>"
assert s_cls.__self_class__ is host_cls

try:
    super(host_cls, 42)
    assert False
except TypeError as e:
    assert str(e) == 'super(type, obj): obj (instance of int) is not an instance or subtype of type (HostPt).'

try:
    object.__new__(host_cls)
    assert False
except TypeError as e:
    assert str(e) == 'object.__new__(HostPt) is not safe, use HostPt.__new__()'

try:
    ValueError.__new__(host_cls)
    assert False
except TypeError as e:
    assert str(e) == 'ValueError.__new__(HostPt): HostPt is not a subtype of ValueError'

1
"#;
    let host_cls = MontyObject::class_type("HostPt", MontyUuid::from_u128(10), true, false, []);
    let host_inst = MontyObject::class_instance(
        MontyObject::class_type("HostPt", MontyUuid::from_u128(10), true, false, []),
        MontyUuid::from_u128(11),
        [],
    );
    let other_cls = MontyObject::class_type("OtherHost", MontyUuid::from_u128(12), true, false, []);
    let mut ex = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec!["host_cls".to_owned(), "host_inst".to_owned(), "other_cls".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let res = ex.run_no_limits(vec![host_cls, host_inst, other_cls]).unwrap();
    assert_eq!(res, MontyObject::int(1));
}

#[test]
fn unhandled_custom_exception_str_resource_limit_is_uncatchable() {
    let width = i64::try_from(isize::MAX as usize / 2 + 1).unwrap();

    let unhandled_code = r"
class OomStrError(Exception):
    def __str__(self):
        return ''.ljust(width, 'é')

raise OomStrError()
";
    let err = MontyRun::new(
        unhandled_code.to_owned(),
        "test.py",
        vec!["width".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![MontyObject::int(width)])
    .unwrap_err();
    assert_eq!(err.exc_type(), ExcType::MemoryError);
    assert_eq!(err.custom_type_name(), None);

    let repr_code = r"
class OomRepr:
    def __repr__(self):
        return ''.ljust(width, 'é')

try:
    repr(OomRepr())
except BaseException:
    pass
";
    let err2 = MontyRun::new(
        repr_code.to_owned(),
        "test.py",
        vec!["width".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![MontyObject::int(width)])
    .unwrap_err();
    assert_eq!(err2.exc_type(), ExcType::MemoryError);
}

#[test]
fn pr4_review_findings_edge_cases() {
    run(r#"
class NewOnly:
    def __new__(cls, *args, **kwargs):
        return object.__new__(cls)

object.__init__(NewOnly(), 1, 2, a=3)
x = NewOnly(1, 2, a=3)
assert isinstance(x, NewOnly)

class InitOnly:
    def __init__(self, *args, **kwargs):
        pass

try:
    object.__init__(InitOnly(), 1, 2)
except TypeError as e:
    assert "object.__init__() takes exactly one argument (the instance to initialize)" in str(e)
else:
    assert False, "expected TypeError on InitOnly"

class Neither:
    pass

try:
    object.__init__(Neither(), 1, 2)
except TypeError as e:
    assert "Neither.__init__() takes exactly one argument (the instance to initialize)" in str(e)
else:
    assert False, "expected TypeError on Neither"

# 2. BaseException.__init__ error message on > 1 args, and non-string arg conversion
e = ValueError("orig")
BaseException.__init__(e, 123)
assert e.args == ("123",)

try:
    BaseException.__init__(e, "a", "b")
except TypeError as err:
    assert "exceptions can only be called with zero or one string argument" in str(err)
else:
    assert False, "expected TypeError on multi-arg BaseException.__init__"

# 3. type.mro on namedtuple
from collections import namedtuple
Pt = namedtuple('Pt', ['x', 'y'])
assert type.mro(Pt) == [Pt, tuple, object]
assert Pt.mro() == [Pt, tuple, object]

# 4. instance __annotations__ isolation
class AnnClass:
    x: int

inst = AnnClass()
assert not hasattr(inst, '__annotations__')
try:
    inst.__annotations__
except AttributeError:
    pass
else:
    assert False, "expected AttributeError for inst.__annotations__"

# Setting explicitly on instance should work
inst.__annotations__ = {'y': 'str'}
assert hasattr(inst, '__annotations__')
assert inst.__annotations__ == {'y': 'str'}
"#);
}
