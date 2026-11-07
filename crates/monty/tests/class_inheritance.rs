use std::time::{Duration, Instant};

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
    run(r#"
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

# An instance's class can be reassigned, as in CPython, but only to a class.
setattr(obj, '__class__', Parent)
assert type(obj) is Parent
object.__setattr__(obj, '__class__', Child)
assert obj.__class__ is Child
try:
    obj.__class__ = 1
except TypeError as e:
    assert str(e) == "__class__ must be set to a class, not 'int' object"
else:
    assert False

obj.__bases__ = Parent
assert obj.__bases__ is Parent

assert obj.__class__ is Child
assert Parent.__bases__ == (object,)
assert Child.__class__ is type
assert Child.__bases__ == (Parent,)
assert isinstance(obj, Parent)
"#);
}

#[test]
fn init_subclass_hooks_and_class_kwargs() {
    run(r#"
class PluginBase:
    registry = []
    def __init_subclass__(cls, plugin_name="default", **kwargs):
        object.__init_subclass__(**kwargs)
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
        object.__init_subclass__(**kwargs)
        calls.append(cls.__name__)

assert calls == []

class SubTracker(Tracker):
    pass

assert calls == ["SubTracker"]

# The nearest hook in the MRO runs
log = []
class A:
    def __init_subclass__(cls, a=1, **kwargs):
        object.__init_subclass__(**kwargs)
        log.append(("A", cls.__name__, a))

class B(A):
    def __init_subclass__(cls, b=2, **kwargs):
        object.__init_subclass__(**kwargs)
        log.append(("B", cls.__name__, b))

assert log == [("A", "B", 1)]

class C(B, b=20):
    pass

assert log == [("A", "B", 1), ("B", "C", 20)]

# Dynamic type() call with kwargs
class DynBase:
    def __init_subclass__(cls, tag="", **kwargs):
        object.__init_subclass__(**kwargs)
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
assert type.mro(type) == [type, object]
try:
    type.mro()
    assert False, 'expected TypeError'
except TypeError as e:
    assert 'unbound method type.mro() needs an argument' in str(e)

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
        return object.__new__(cls)

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
        Base.__init__(self, x)
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

    // Repl session snapshotting and restoring class hierarchy
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        r"
class A:
    def val(self):
        return 10

class B(A):
    def val(self):
        return A.val(self) + 20

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

/// The C3 merge is cubic in the number of bases, so it must poll the deadline.
#[test]
fn mro_merge_respects_time_limits() {
    let code = r"
bases = tuple(type(f'B{i}', (), {}) for i in range(3000))
type('C', bases, {})
";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(200));
    let start = Instant::now();
    let exc = ex
        .run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    // Unpolled, the merge runs for over a minute before the limit is seen.
    assert!(start.elapsed() < Duration::from_secs(10), "took {:?}", start.elapsed());
}

#[test]
fn instance_bases_reassignment_is_an_instance_attribute() {
    run(r"
class Foo:
    pass

f = Foo()
f.__bases__ = 42
assert f.__bases__ == 42

");
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
        object.__init_subclass__()
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
fn custom_new_subclass_init_and_dataclass() {
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
assert type.mro(type) == [type, object]
try:
    type.mro()
    assert False, 'expected TypeError'
except TypeError as e:
    assert 'unbound method type.mro() needs an argument' in str(e)

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
from dataclasses import FrozenInstanceError
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

");

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
    return Base.m1(self) + 2

# Base has same function bound to multiple names
Base = type('Base', (), {'m1': f, 'm2': f, 'm3': f})
Child = type('Child', (Base,), {'m1': g, 'm2': g})

c = Child()
assert c.m1() == 3
assert c.m2() == 3
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
fn descriptor_parity_and_hasattr() {
    run(r"
class A:
    pass


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
        return object.__new__(cls)

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
fn class_cell_and_subclass_ops_survive_dump_restore() {
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
            return (lambda s: cls.val(s) + 1)(self)
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
fn host_class_type_inheritance_issubclass_and_new() {
    let code = r"
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


try:
    object.__new__(host_cls)
    assert False
except TypeError as e:
    assert str(e) == 'object.__new__(HostPt) is not safe, use HostPt.__new__()'

1
";
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

#[test]
fn type_duplicate_bases_large_tuple_does_not_oom() {
    run(r"
try:
    type('C', (object,) * 100_000, {})
    assert False, 'should have raised TypeError'
except TypeError as e:
    assert 'duplicate base class object' in str(e)
");
}

#[test]
fn large_class_name_round_trips_through_type() {
    run(r"
name = 'X' * 1_000_000
C = type(name, (), {})
assert C.__name__ == name
assert type(C()) is C
");
}

#[test]
fn class_annotations_call_syntax_no_panic() {
    run_err(
        r"
class Foo:
    x: int

Foo.__annotations__([1, 2])
",
        "'dict' object is not callable",
    );
}

#[test]
fn type_mro_unbound_descriptor() {
    run(r"
class A:
    pass

assert type.mro(type) == [type, object]
assert type.mro(A) == [A, object]

m = type.mro
assert m(A) == [A, object]
assert m(int) == [int, object]
");
    run_err("type.mro()", "unbound method type.mro() needs an argument");
    run_err("getattr(type, 'mro')()", "unbound method type.mro() needs an argument");
}

#[test]
fn annotate_func_subclass_is_none() {
    run(r"
class AnnBase:
    x: int

class AnnSub(AnnBase):
    pass

assert AnnBase.__annotate_func__ is not None
assert AnnSub.__annotate_func__ is None
");
}

#[test]
fn instance_annotations_and_custom_class_lookup() {
    run(r#"
AnnExplicit = type('AnnExplicit', (), {'__annotations__': {'a': int}})
inst = AnnExplicit()

# An `__annotations__` entry in the namespace is an ordinary class attribute
assert inst.__annotations__ == {'a': int}
try:
    inst.__annotations__()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'dict' object is not callable"

class C:
    __class__ = lambda self: 'custom'

c = C()
assert c.__class__() == 'custom'
assert (c.__class__)() == 'custom'
assert type(c) is C
"#);
}

#[test]
fn object_init_excess_args_and_object_setattr() {
    run(r"
assert object.__init__(1, 2) is None
assert object.__init__('hello', k=1) is None
assert object.__init__((1, 2), 'extra') is None

class ObjectSet(object):
    def set_val(self, k, v):
        object.__setattr__(self, k, v)

s = ObjectSet()
s.set_val('x', 99)
assert s.x == 99
");
    run_err(
        "object.__init__(ValueError('orig'), 1)",
        "object.__init__() takes exactly one argument (the instance to initialize)",
    );
    run_err(
        "object.__init__([1, 2], 'extra')",
        "object.__init__() takes exactly one argument (the instance to initialize)",
    );
}

#[test]
fn custom_class_constructor_args_memory_preflight() {
    let code = r"
class Base:
    def __new__(cls, *args, **kwargs):
        return object.__new__(cls)

args = (1,) * 200_000
Base(*args)
";
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_memory(500 * 1024);
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_err());
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}
