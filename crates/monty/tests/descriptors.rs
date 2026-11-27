//! Descriptor binding, suspension and persistence regressions.

use std::time::Duration;

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, ReplProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, MontyUuid, NameLookupResult, PrintWriter,
    ResourceLimits, ResourceTracker,
};

/// Start an independent session for suspension and persistence checks.
fn repl() -> MontyRepl {
    MontyRepl::new("descriptors.py", ResourceTracker::default(), CompileOptions::default())
}

/// Reload a suspended frame before its next host response.
fn restore(progress: ReplProgress) -> ReplProgress {
    let bytes = dump("descriptors.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    drop(progress.into_repl());
    let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("expected suspended state")
    };
    *progress
}

/// Reload an idle session so native callables and guards are exercised after deserialization.
fn restore_idle(repl: MontyRepl) -> MontyRepl {
    let bytes = dump("descriptors.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    drop(repl);
    let Session::Idle(repl) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle state")
    };
    *repl
}

/// Forwarded annotation getters retain the caller's fallback across a host error and restore.
#[test]
fn bound_method_attribute_errors_survive_suspension_and_restore() {
    let code = r"
class Ann:
    __annotations__ = property(host_get)
class C:
    m = classmethod(Ann())

assert getattr(C.m, '__annotations__', ['default']) == ['default']
assert not hasattr(C.m, '__annotations__')
M = type(C.m)
match C.m:
    case M(__annotations__=value):
        assert False, 'expected a missing attribute'
    case _:
        pass
try:
    getattr(C.m, '__annotations__')
    assert False, 'expected AttributeError without a default'
except AttributeError as error:
    assert str(error) == 'from host'
42
";
    for name in ["__annotations__", "__annotate__"] {
        for getter in ["host_get", "lambda obj: host_get(obj)"] {
            let code = code
                .replace("__annotations__", name)
                .replace("property(host_get)", &format!("property({getter})"));
            let mut progress = repl().feed_start(&code, vec![], PrintWriter::Disabled).unwrap();
            let mut calls = 0;
            loop {
                progress = match restore(progress) {
                    ReplProgress::NameLookup(lookup) => {
                        let function = MontyObject::function(lookup.name.clone(), None);
                        lookup
                            .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                            .unwrap()
                    }
                    ReplProgress::FunctionCall(call) => {
                        calls += 1;
                        let error = MontyException::new(ExcType::AttributeError, Some("from host".to_owned()));
                        call.resume(ExtFunctionResult::Error(error), PrintWriter::Disabled)
                            .unwrap()
                    }
                    complete => {
                        assert_eq!(complete.into_complete().unwrap().1, MontyObject::int(42));
                        break;
                    }
                };
            }
            assert_eq!(calls, 4);
        }
    }
}

/// OS calls from a forwarded getter suspend for all three attribute consumers.
#[test]
fn bound_method_attribute_os_calls_survive_restore() {
    let code = r"
import functools
import os

class Ann:
    __annotations__ = property(functools.partial(os.getenv, 'HOME'))
class C:
    m = classmethod(Ann())

assert hasattr(C.m, '__annotations__')
assert getattr(C.m, '__annotations__', None) == 'sandbox home'
M = type(C.m)
match C.m:
    case M(__annotations__=value):
        assert value == 'sandbox home'
    case _:
        assert False, 'expected a matching attribute'
42
";
    let mut progress = repl().feed_start(code, vec![], PrintWriter::Disabled).unwrap();
    let mut calls = 0;
    loop {
        progress = match restore(progress) {
            ReplProgress::OsCall(call) => {
                calls += 1;
                assert_eq!(call.function_call.name(), "os.getenv");
                call.resume(MontyObject::string("sandbox home"), PrintWriter::Disabled)
                    .unwrap()
            }
            complete => {
                assert_eq!(complete.into_complete().unwrap().1, MontyObject::int(42));
                break;
            }
        };
    }
    assert_eq!(calls, 3);
}

/// A forwarded native getter can install its own host-lookup default, which the outer read must retain.
#[test]
fn forwarded_host_lookup_defaults_survive_restore() {
    let host = MontyObject::class_instance(
        MontyObject::class_type("Host", MontyUuid::from_u128(7), true, false, []),
        MontyUuid::from_u128(42),
        [],
    );
    let code = r"
import functools
class Ann:
    __annotations__ = property(functools.partial(getattr, host, 'missing'))
class C:
    m = classmethod(Ann())

assert type(getattr(C.m, '__annotations__')) is Ann
assert type(getattr(C.m, '__annotations__', 'outer default')) is Ann
assert hasattr(C.m, '__annotations__')
M = type(C.m)
match C.m:
    case M(__annotations__=value):
        assert type(value) is Ann
    case _:
        assert False, 'expected the inner default'
42
";
    let mut progress = repl()
        .feed_start(code, vec![("host".to_owned(), host)], PrintWriter::Disabled)
        .unwrap();
    let mut lookups = 0;
    loop {
        progress = match restore(progress) {
            ReplProgress::NameLookup(lookup) => {
                lookups += 1;
                assert_eq!(lookup.name, "missing");
                assert_eq!(lookup.object_id(), Some(MontyUuid::from_u128(42)));
                lookup
                    .resume(NameLookupResult::Undefined, PrintWriter::Disabled)
                    .unwrap()
            }
            complete => {
                assert_eq!(complete.into_complete().unwrap().1, MontyObject::int(42));
                break;
            }
        };
    }
    assert_eq!(lookups, 4);
}

/// Native descriptor cycles must hit recursion accounting before the time-limit backstop.
#[test]
fn descriptor_call_cycles_are_bounded() {
    let limits = ResourceLimits::default()
        .max_recursion_depth(8)
        .max_feed_duration(Duration::from_secs(5));
    let mut repl = MontyRepl::new(
        "descriptors.py",
        ResourceTracker::new(limits),
        CompileOptions::default(),
    );
    let error = repl
        .feed_run(
            "class C: pass\nc = C()\nC.__call__ = staticmethod(c)\nc()",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::RecursionError);
    assert_snapshot!(error.message().unwrap(), @"maximum recursion depth exceeded");
    assert_eq!(
        repl.feed_run("1 + 1", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::int(2)
    );
    let mut repl = restore_idle(repl);
    repl.feed_run(
        r"
for wrapper in (staticmethod(c), classmethod(c), property(lambda self: c)):
    for _ in range(2):
        C.__call__ = wrapper
        try:
            c()
            assert False, 'expected RecursionError'
        except RecursionError as error:
            assert str(error) == 'maximum recursion depth exceeded'
        C.__call__ = staticmethod(lambda: 42)
        assert c() == 42
assert property(len).__get__([1, 2]) == 2
",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
}

/// Host turns and dump/restore must not reset an implementation-only cycle's frame budget.
#[test]
fn descriptor_cycle_budget_survives_suspension_and_restore() {
    let limits = ResourceLimits::default()
        .max_recursion_depth(1)
        .max_feed_duration(Duration::from_secs(5));
    let repl = MontyRepl::new(
        "descriptors.py",
        ResourceTracker::new(limits),
        CompileOptions::default(),
    );
    let mut progress = repl
        .feed_start(
            "class Gate: pass\nGate.__get__ = hook\nclass C:\n    __call__ = Gate()\nc = C()\nc()",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
    let mut calls = 0;
    loop {
        if matches!(calls, 4 | 512) {
            progress = restore(progress);
        }
        progress = match progress {
            ReplProgress::NameLookup(lookup) => {
                let function = MontyObject::function(lookup.name.clone(), None);
                lookup
                    .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                    .unwrap()
            }
            ReplProgress::FunctionCall(call) => {
                calls += 1;
                assert!(calls <= 1024, "suspension must not reset the internal-frame cap");
                let receiver = call
                    .args
                    .args()
                    .nth(1)
                    .expect("getter received its instance")
                    .to_owned();
                match call.resume(receiver, PrintWriter::Disabled) {
                    Ok(progress) => progress,
                    Err(error) => {
                        assert_eq!(error.error.exc_type(), ExcType::RecursionError);
                        assert_snapshot!(error.error.message().unwrap(), @"maximum recursion depth exceeded");
                        break;
                    }
                }
            }
            _ => panic!("descriptor call cycle completed without a recursion error"),
        };
    }
    assert_eq!(calls, 1024);
}

#[test]
fn mixed_class_lookup_survives_restore_and_suspension() {
    let mut repl = repl();
    repl.feed_run(
        r"
class Record:
    __match_args__ = ('x',)
    @staticmethod
    def method():
        return 1
record = Record()
record.x = 7
",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let bytes = dump("descriptors.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(repl) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle state")
    };
    let mut progress = repl
        .feed_start(
            r"
assert record.x == 7
Record.method = lambda self: self.x
assert record.method() == 7
record.method = lambda: 9
assert record.method() == 9
Record.computed = property(host_get)
assert getattr(record, 'x') == 7
assert not hasattr(record, 'absent')
assert record.computed == 42
del record.method
Record.method = lambda self: host_get()
assert record.method() == 42

def suspended(self, value=0):
    return host_get() + value

Record.method = suspended
assert record.method(value=1) == 43
record.method = lambda: 9
Record.method = property(lambda self: host_get)
assert record.method() == 42
match record:
    case Record(x):
        assert x == 7
Record.x = property(host_get)
match record:
    case Record(x):
        assert x == 42
42
",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
    let mut calls = 0;
    loop {
        match restore(progress) {
            ReplProgress::NameLookup(lookup) => {
                let function = MontyObject::function(lookup.name.clone(), None);
                progress = lookup
                    .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                    .unwrap();
            }
            ReplProgress::FunctionCall(call) => {
                assert_eq!(call.function_name, "host_get");
                calls += 1;
                progress = call.resume(MontyObject::int(42), PrintWriter::Disabled).unwrap();
            }
            complete => {
                assert_eq!(complete.into_complete().unwrap().1, MontyObject::int(42));
                assert_eq!(calls, 5);
                break;
            }
        }
    }
}

#[test]
fn restored_plain_classes_can_gain_suspending_descriptors() {
    let mut repl = repl();
    repl.feed_run(
        "class Record:\n    x = 7\nrecord = Record()\nassert record.x == 7",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let bytes = dump("descriptors.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(repl) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle state")
    };
    let mut progress = repl
        .feed_start(
            "Record.x = property(host_get)\ngetattr(record, 'x', 99)",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
    loop {
        match restore(progress) {
            ReplProgress::NameLookup(lookup) => {
                let function = MontyObject::function(lookup.name.clone(), None);
                progress = lookup
                    .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                    .unwrap();
            }
            ReplProgress::FunctionCall(call) => {
                assert_eq!(call.function_name, "host_get");
                progress = call.resume(MontyObject::int(42), PrintWriter::Disabled).unwrap();
            }
            complete => {
                assert_eq!(complete.into_complete().unwrap().1, MontyObject::int(42));
                break;
            }
        }
    }
}

#[test]
fn native_property_getter_chains_are_bounded_and_release_the_recursion_guard() {
    let mut repl = repl();
    assert_eq!(
        repl.feed_run(
            r"
p = property(len)
for _ in range(100):
    p = property(p.__get__)
for _ in range(2):
    try:
        p.__get__([])
    except RecursionError:
        pass
    else:
        assert False
assert property(len).__get__([1, 2]) == 2
42
",
            vec![],
            PrintWriter::Disabled
        )
        .unwrap(),
        MontyObject::int(42)
    );
    let mut repl = restore_idle(repl);
    assert_eq!(
        repl.feed_run("1 + 1", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::int(2)
    );
}

#[test]
fn dataclass_descriptor_setters_reject_suspension_without_corrupting_state() {
    let mut repl = repl();
    repl.feed_run(
        r#"
import dataclasses
import os

@dataclasses.dataclass
class Record:
    x: int

def external_setter(obj, value):
    ext_fn(value)

def os_setter(obj, value):
    os.path.exists('/value')

class ExternalField:
    def __set__(self, obj, value):
        ext_fn(value)

class OsField:
    def __set__(self, obj, value):
        os.path.exists('/value')

for descriptor, kind, name in (
    (property(None, external_setter), 'external function', 'ext_fn'),
    (property(None, os_setter), 'OS function', 'Path.exists'),
    (ExternalField(), 'external function', 'ext_fn'),
    (OsField(), 'OS function', 'Path.exists'),
):
    Record.x = descriptor
    for _ in range(2):
        try:
            Record(42)
        except NotImplementedError as exc:
            assert str(exc) == f"{kind} '{name}' cannot suspend in this synchronous context"
        else:
            assert False

def local_setter(obj, value):
    obj.actual = value

Record.x = property(None, local_setter)
assert Record(42).actual == 42
"#,
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    assert_eq!(
        repl.feed_run("Record(7).actual", vec![], PrintWriter::Disabled)
            .unwrap(),
        MontyObject::int(7)
    );
}

#[test]
fn every_descriptor_hook_survives_suspension_and_restore() {
    let code = r"
class Field:
    def __set_name__(self, owner, name):
        ext_fn('name', name)
        self.name = '_' + name

    def __get__(self, obj, owner):
        if obj is None:
            return self
        ext_fn('get')
        return getattr(obj, self.name)

    def __set__(self, obj, value):
        ext_fn('set', value)
        setattr(obj, self.name, value)
        return 123

    def __delete__(self, obj):
        ext_fn('delete')
        delattr(obj, self.name)
        return 456

class Record:
    field = Field()

    @property
    def prop(self):
        ext_fn('property get')
        return self._prop

    @prop.setter
    def prop(self, value):
        ext_fn('property set', value)
        self._prop = value
        return 789

    @prop.deleter
    def prop(self):
        ext_fn('property delete')
        del self._prop

    @classmethod
    def make(cls):
        ext_fn('classmethod')
        return cls()

    @staticmethod
    def plus(value):
        ext_fn('staticmethod')
        return value + 1

r = Record.make()
r.field = 7
assert r.field == 7
assert getattr(r, 'field') == 7
assert hasattr(r, 'field')
del r.field
assert getattr(r, 'field', 99) == 99
assert not hasattr(r, 'field')
r.prop = 8
assert r.prop == 8
assert r.plus(8) == 9
del r.prop
42
";
    let mut progress = repl().feed_start(code, vec![], PrintWriter::Disabled).unwrap();
    let mut stages = Vec::new();
    loop {
        let loaded = restore(progress);
        match loaded {
            ReplProgress::NameLookup(lookup) => {
                let function = MontyObject::function(lookup.name.clone(), None);
                progress = lookup
                    .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                    .unwrap();
            }
            ReplProgress::FunctionCall(call) => {
                assert_eq!(call.function_name, "ext_fn");
                stages.push(call.args.args().next().unwrap().as_str().unwrap().to_owned());
                progress = call.resume(MontyObject::none(), PrintWriter::Disabled).unwrap();
            }
            other => {
                let (_repl, value) = other.into_complete().expect("expected completion");
                assert_eq!(value, MontyObject::int(42));

                break;
            }
        }
    }
    assert_eq!(
        stages,
        [
            "name",
            "classmethod",
            "set",
            "get",
            "get",
            "get",
            "delete",
            "get",
            "get",
            "property set",
            "property get",
            "staticmethod",
            "property delete"
        ]
    );
}

#[test]
fn suspended_getter_errors_and_class_patterns_restore() {
    let mut repl = repl();
    repl.feed_run(
        "class Item:\n    @property\n    def x(self):\n        return ext_fn()\nitem = Item()",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    for (code, missing, expected) in [
        ("getattr(item, 'x', 99)", true, MontyObject::int(99)),
        ("hasattr(item, 'x')", true, MontyObject::bool(false)),
        ("hasattr(item, 'x')", false, MontyObject::bool(true)),
        (
            "match item:\n    case Item(x=value):\n        result = value\n    case _:\n        result = -1\nresult",
            false,
            MontyObject::int(42),
        ),
        (
            "match item:\n    case Item(x=value):\n        result = value\n    case _:\n        result = -1\nresult",
            true,
            MontyObject::int(-1),
        ),
    ] {
        let progress = repl.feed_start(code, vec![], PrintWriter::Disabled).unwrap();
        let progress = restore(progress);
        let call = progress.into_function_call().unwrap();
        let result = if missing {
            monty_types::ExtFunctionResult::Error(monty_types::MontyException::new(
                monty_types::ExcType::AttributeError,
                Some("missing".to_owned()),
            ))
        } else {
            monty_types::ExtFunctionResult::Return(MontyObject::int(42))
        };
        let progress = call.resume(result, PrintWriter::Disabled).unwrap();
        let (next, value) = progress.into_complete().unwrap();
        assert_eq!(value, expected);
        repl = next;
    }
    let bytes = dump("descriptors.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut repl) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle state")
    };
    assert_eq!(
        repl.feed_run("Item.x.__name__", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::string("x")
    );
}

#[test]
fn property_os_call_survives_restore() {
    let progress = repl().feed_start("import os\nclass Item:\n    @property\n    def exists(self):\n        return os.path.exists('/value')\nItem().exists", vec![], PrintWriter::Disabled).unwrap();
    let progress = restore(progress);
    let ReplProgress::OsCall(call) = progress else {
        panic!("expected an OS call")
    };
    let (_, value) = call
        .resume(MontyObject::bool(true), PrintWriter::Disabled)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(value, MontyObject::bool(true));
}

#[test]
fn native_property_getter_getattr_survives_restore() {
    let code = r"
class Record:
    x = property(host_get)
r = Record()
assert r.x == 42
assert getattr(r, 'x') == 42
assert getattr(r, 'x', 99) == 42
42
";
    let mut progress = repl().feed_start(code, vec![], PrintWriter::Disabled).unwrap();
    let mut calls = 0;
    loop {
        match restore(progress) {
            ReplProgress::NameLookup(lookup) => {
                let function = MontyObject::function(lookup.name.clone(), None);
                progress = lookup
                    .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                    .unwrap();
            }
            ReplProgress::FunctionCall(call) => {
                assert_eq!(call.function_name, "host_get");
                calls += 1;
                progress = call.resume(MontyObject::int(42), PrintWriter::Disabled).unwrap();
            }
            complete => {
                assert_eq!(complete.into_complete().unwrap().1, MontyObject::int(42));
                break;
            }
        }
    }
    assert_eq!(calls, 3);
}

#[test]
fn descriptor_match_args_survives_suspension_and_restore() {
    for missing in [false, true] {
        let code = r"
class Names:
    def __get__(self, obj, owner):
        assert obj is None
        assert owner is Record
        metadata()
        return ('x',)
class Record:
    __match_args__ = Names()
    @property
    def x(self):
        return value()
    y = 8
match Record():
    case Record(x, y=y):
        result = (x, y)
    case _:
        result = None
result
";
        let mut progress = repl().feed_start(code, vec![], PrintWriter::Disabled).unwrap();
        let mut calls = Vec::new();
        loop {
            match restore(progress) {
                ReplProgress::NameLookup(lookup) => {
                    let function = MontyObject::function(lookup.name.clone(), None);
                    progress = lookup
                        .resume(NameLookupResult::Value(function), PrintWriter::Disabled)
                        .unwrap();
                }
                ReplProgress::FunctionCall(call) => {
                    calls.push(call.function_name.clone());
                    let result = if missing && call.function_name == "metadata" {
                        monty_types::ExtFunctionResult::Error(monty_types::MontyException::new(
                            monty_types::ExcType::AttributeError,
                            Some("missing metadata".to_owned()),
                        ))
                    } else {
                        monty_types::ExtFunctionResult::Return(MontyObject::int(42))
                    };
                    match call.resume(result, PrintWriter::Disabled) {
                        Ok(next) => progress = next,
                        Err(error) => {
                            assert!(missing);
                            assert_snapshot!(error.error.message().unwrap(), @"Record() accepts 0 positional sub-patterns (1 given)");
                            break;
                        }
                    }
                }
                complete => {
                    assert!(!missing);
                    assert_eq!(
                        complete.into_complete().unwrap().1,
                        MontyObject::tuple(vec![MontyObject::int(42), MontyObject::int(8)])
                    );
                    break;
                }
            }
        }
        assert_eq!(
            calls,
            if missing {
                vec!["metadata"]
            } else {
                vec!["metadata", "value"]
            }
        );
    }
}

#[test]
fn nested_classmethod_bindings_are_bounded_and_release_the_recursion_guard() {
    let mut repl = repl();
    repl.feed_run(
        r"
f = len
for _ in range(1000):
    f = classmethod(f).__get__(None, int)
for _ in range(2):
    try:
        f()
    except RecursionError:
        pass
    else:
        assert False
assert classmethod(len).__get__(None, [1, 2])() == 2
",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let mut repl = restore_idle(repl);
    assert_eq!(
        repl.feed_run("1 + 1", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::int(2)
    );
}

#[test]
fn descriptor_protocol_methods_reject_deepcopy_replaced_receivers() {
    let mut repl = repl();
    repl.feed_run(
        r"
import copy
p = property(len)
forged_methods = []
for method, args in ((p.__get__, ([],)), (p.__set__, ([], 1)), (p.__delete__, ([],)), (p.__set_name__, (int, 'x'))):
    for receiver in ([], {}, 42):
        forged_methods.append((copy.deepcopy(method, {id(p): receiver}), args))
",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let mut repl = restore_idle(repl);
    repl.feed_run(
        r"
for forged, args in forged_methods:
    try:
        forged(*args)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'invalid descriptor receiver'
assert p.__get__([1, 2]) == 2
",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
}
