use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

fn run(source: &str) {
    MontyRun::new(source.to_owned(), "native_test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap();
}

#[test]
fn custom_new_controls_allocation_and_initialization() {
    run(r"
events=[]
class Parent:
    def __new__(cls, value):
        events.append('new')
        result=object.__new__(cls)
        result.from_new=value
        return result
    def __init__(self,value):
        events.append('init')
        self.value=value
class Child(Parent): pass
child=Child(7)
assert type(child) is Child
assert child.from_new == child.value == 7
assert events == ['new','init']
assert child.__new__(Child,8).__class__ is Child

class ReturnsNumber:
    def __new__(cls): return 42
    def __init__(self): assert False
assert ReturnsNumber() == 42

class NewOnly:
    def __new__(cls, value):
        result=object.__new__(cls)
        result.value=value
        return result
assert NewOnly(5).value == 5

class Other: pass
class ReturnsOther:
    def __new__(cls): return Other()
    def __init__(self): assert False
assert type(ReturnsOther()) is Other
");
}

#[test]
fn native_list_storage_survives_custom_initialization_and_indirect_inheritance() {
    run(r"
class XS(list):
    def __init__(self): self.calls=[]
    def append(self,value):
        self.calls.append(value)
        list.append(self,value)
class Child(XS): pass
xs=Child()
xs.append(1)
list.append(xs,2)
assert xs == [1,2]
assert [1,2] == xs
assert xs.calls == [1]
assert type(xs) is Child
assert isinstance(xs, XS)
assert isinstance(xs,list)
assert isinstance(xs,(dict,list))
assert issubclass(Child,list)
assert Child.__bases__ == (XS,)
assert XS.__bases__ == (list,)
assert len(xs) == 2
assert xs[1] == 2
xs[0]=3
assert 3 in xs
assert list(xs) == [3,2]
assert type(xs[:]) is list
assert type(xs.copy()) is list
xs.extend([4,5])
assert xs.pop() == 5
assert xs.count(4) == 1
assert xs.index(4) == 2
xs.reverse()
xs.sort()
assert xs == [2,3,4]
assert repr(xs) == '[2, 3, 4]'
assert not XS()

class Inherited(list): pass
assert Inherited([7,8]) == [7,8]
class Explicit(list):
    def __init__(self,values): list.__init__(self,values)
assert Explicit([9]) == [9]
class CustomNew(list):
    def __new__(cls,values): return list.__new__(cls)
assert CustomNew([6]) == [6]
class Length(list):
    def __len__(self): return 99
length=Length([1,2])
assert len(length) == 99
assert list.__len__(length) == 2
");
}

#[test]
fn classes_and_bases_cannot_be_reassigned() {
    run(r"
class Parent: pass
class Child(Parent): pass
obj=Child()
for target in [Parent, Child, obj]:
    for name in ['__class__','__bases__']:
        try: setattr(target,name,Parent)
        except TypeError: pass
        else: assert False
try: object.__setattr__(obj,'__class__',Parent)
except TypeError: pass
else: assert False
assert obj.__class__ is Child
assert Parent.__bases__ == (object,)
assert Child.__class__ is type
assert Child.__bases__ == (Parent,)
assert isinstance(obj,Parent)
");
}

#[test]
fn native_exception_ancestry_and_identity_are_preserved() {
    run(r#"
class Error(ValueError): pass
class Child(Error): pass
error=Child('message',[1,2])
assert type(error) is Child
assert error.args == ('message',[1,2])
assert isinstance(error,ValueError)
assert isinstance(error,Exception)
assert isinstance(error,Error)
assert issubclass(Child,ValueError)
assert Child.__bases__ == (Error,)
assert Error.__bases__ == (ValueError,)
try:
    raise error
except Error as caught:
    assert caught is error
    try: raise
    except ValueError as reraised: assert reraised is error
else: assert False
try: raise Child
except Exception as caught: assert type(caught) is Child
else: assert False

class Custom(Exception):
    def __new__(cls,message): return Exception.__new__(cls,message)
    def __init__(self,message): self.message=message
custom=Custom('hello')
assert custom.args == ('hello',)
assert str(custom) == 'hello'
assert repr(custom) == "Custom('hello')"
try: raise custom
except Custom as caught: assert caught is custom
else: assert False

class Context:
    def __enter__(self): return self
    def __exit__(self,typ,value,traceback):
        assert typ is Child
        assert value is error
        return True
with Context(): raise error

Dynamic=type('Dynamic',(Error,),{})
assert isinstance(Dynamic(),ValueError)
"#);
}

#[test]
fn unrelated_native_layouts_and_receivers_are_rejected() {
    run(r"
class Plain: pass
for native in [int,dict,tuple]:
    try: type('Unsupported',(native,),{})
    except TypeError: pass
    else: assert False
try: type('Conflict',(list,Exception),{})
except TypeError: pass
else: assert False
for operation in [lambda: list.append(Plain(),1),lambda: list.__new__(Plain),lambda: Exception.__new__(Plain)]:
    try: operation()
    except TypeError: pass
    else: assert False
");
}

#[test]
fn native_instances_round_trip_through_idle_snapshots() {
    let mut repl = MontyRepl::new("native_test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "class XS(list): pass\nxs=XS([1,2])\nclass Error(Exception): pass\nerror=Error('message')",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("native_test.py", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle session")
    };
    let output=restored.feed_run("assert type(xs) is XS\nassert isinstance(xs,list)
assert isinstance(xs,(dict,list))\nxs.append(3)\nassert xs == [1,2,3]\ntry:\n    raise error\nexcept Error as caught:\n    assert caught is error\nlen(xs)",vec![],PrintWriter::Stdout).unwrap();
    assert_eq!(output, MontyObject::int(3));
}

#[test]
#[cfg(feature = "ref-count-return")]
fn native_storage_cycles_and_failed_new_calls_release_references() {
    let source = r"
def exercise():
    class XS(list): pass
    xs=XS()
    xs.append(xs)
    class Error(Exception): pass
    error=Error()
    error.args=(error,)
    class Broken(XS):
        def __new__(cls, value): raise ValueError('failed')
    try: Broken(xs)
    except ValueError: pass
for _ in range(10): exercise()
";
    let output = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_ref_counts(vec![])
        .unwrap();
    assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
}

#[test]
fn object_is_the_root_of_custom_and_native_classes() {
    run(r"
class Parent: pass
class Child(Parent): pass
class Items(list): pass
class SubItems(Items): pass
class Error(ValueError): pass
assert object.__bases__ == ()
assert Parent.__bases__ == (object,)
assert Child.__bases__ == (Parent,)
assert list.__bases__ == (object,)
assert Items.__bases__ == (list,)
assert SubItems.__bases__ == (Items,)
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

for cls in (Parent, Child, list, Items, SubItems, Error, ValueError, Exception, BaseException, bool, int, dict, tuple, str, type):
    assert issubclass(cls, object)
    assert cls.__class__ is type
for value in (Parent(), Child(), [], Items(), SubItems(), Error('x'), ValueError('x'), True, 1, {}, (), 'x', object):
    assert isinstance(value, object)
");
}
