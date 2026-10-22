//! Descriptor binding, suspension and persistence regressions.

use monty::{Dump, MontyRepl, ReplProgress, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, NameLookupResult, PrintWriter, ResourceTracker};

fn repl() -> MontyRepl {
    MontyRepl::new("descriptors.py", ResourceTracker::default(), CompileOptions::default())
}

fn restore(progress: ReplProgress) -> ReplProgress {
    let bytes = dump("descriptors.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    drop(progress.into_repl());
    let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("expected suspended state")
    };
    *progress
}

#[test]
fn descriptor_protocol_and_builtin_wrappers() {
    repl()
        .feed_run(
            include_str!("../test_cases/class__descriptors.py"),
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
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
                            assert!(error.error.to_string().contains("accepts 0 positional sub-patterns"));
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
fn descriptor_match_args_validation_and_short_circuits() {
    repl()
        .feed_run(
            r"
class Names:
    def __get__(self, obj, owner):
        assert obj is None
        return owner.names
class Record:
    __match_args__ = Names()
    names = ('x', 123)
    x = 42

# Unused tuple elements do not need to be strings.
match Record():
    case Record(x):
        assert x == 42

Record.names = []
# Neither a failed instance check nor a keyword-only pattern reads metadata.
match 42:
    case Record(x):
        assert False
match Record():
    case Record(x=x):
        assert x == 42

try:
    match Record():
        case Record(x):
            pass
except TypeError as e:
    assert '__match_args__ must be a tuple' in str(e)
else:
    assert False

Record.names = (123,)
try:
    match Record():
        case Record(x, y):
            pass
except TypeError as e:
    assert 'accepts 1 positional sub-pattern' in str(e)
else:
    assert False
try:
    match Record():
        case Record(x):
            pass
except TypeError as e:
    assert '__match_args__ elements must be strings' in str(e)
else:
    assert False
",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap();
}
