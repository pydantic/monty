//! Runtime tests for the `collections` module's importable surface.
//!
//! These cannot be datatest `test_cases` (which dual-run against CPython): the
//! whole point is that Monty *diverges* from CPython here — CPython imports
//! `OrderedDict` / `ChainMap` / `UserDict` fine, whereas Monty raises
//! `ImportError` because it implements only `deque`, `Counter`, `defaultdict`,
//! and `namedtuple`. The narrowed typeshed stub
//! (`crates/monty-typeshed/custom/collections/__init__.pyi`) makes the same
//! names a type error; these tests pin the matching *runtime* behaviour so the
//! two stay in agreement.
//!
//! The cycle-collection tests at the bottom likewise can't be `test_cases`:
//! they drive `gc.collect()`, which is a `test-hooks`-only module deliberately
//! kept out of the sandbox surface.

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyObject};

/// Runs `from collections import <name>` and returns the raised exception.
fn import_err(name: &str) -> monty_types::MontyException {
    let code = format!("from collections import {name}");
    let run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("should parse");
    run.run_no_limits(vec![]).expect_err("expected ImportError")
}

/// The four members Monty implements import cleanly.
#[test]
fn implemented_names_import() {
    let run = MontyRun::new(
        "from collections import deque, Counter, defaultdict, namedtuple".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("should parse");
    assert!(
        run.run_no_limits(vec![]).is_ok(),
        "the implemented collections members should import without error",
    );
}

/// `OrderedDict` is not implemented, so importing it raises `ImportError` with a
/// CPython-shaped message (Monty diverges here — CPython would import it).
#[test]
fn ordereddict_import_raises() {
    let err = import_err("OrderedDict");
    assert_eq!(err.exc_type(), ExcType::ImportError);
    assert_snapshot!(
        err.message().unwrap(),
        @"cannot import name 'OrderedDict' from 'collections' (unknown location)"
    );
}

/// `ChainMap` is likewise unimplemented.
#[test]
fn chainmap_import_raises() {
    let err = import_err("ChainMap");
    assert_eq!(err.exc_type(), ExcType::ImportError);
    assert_snapshot!(
        err.message().unwrap(),
        @"cannot import name 'ChainMap' from 'collections' (unknown location)"
    );
}

/// The `UserDict` / `UserList` / `UserString` wrappers depend on subclassing,
/// which Monty has no support for, so none of them are importable.
#[test]
fn user_wrappers_import_raises() {
    for name in ["UserDict", "UserList", "UserString"] {
        let err = import_err(name);
        assert_eq!(err.exc_type(), ExcType::ImportError, "{name} should raise ImportError");
        assert_eq!(
            err.message().unwrap(),
            format!("cannot import name '{name}' from 'collections' (unknown location)"),
        );
    }
}

/// A namedtuple instance owns a counted reference to its class object, so a
/// cycle routed through that edge (instance → class → `defaults` → instance)
/// must be collectable.
///
/// This pins the `for_each_child_id` / `py_dec_ref_ids` invariant for
/// `HeapData::NamedTuple`: the instance's `contains_refs` flag is computed from
/// its *items* alone, and here the only item is `0`, so the class edge is the
/// sole thing keeping the cycle alive. If the collector's trace skips
/// `class_id`, nothing here is reclaimable.
#[test]
#[cfg(feature = "test-hooks")]
fn namedtuple_class_cycle_is_collected() {
    let code = r"
import gc
from collections import namedtuple

def build():
    values = []
    cls = namedtuple('C', ['a'], defaults=[values])
    # items = [0] holds no heap refs, so `contains_refs` is False
    values.append(cls(0))

build()
gc.collect()
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let freed = run.run_no_limits(vec![]).expect("should run");
    let MontyObject::Int(freed) = freed else {
        panic!("gc.collect() should return an int, got {freed:?}");
    };
    assert!(
        freed > 0,
        "the instance -> class -> defaults cycle should be collected, but gc.collect() freed {freed} entries",
    );
}

/// A cycle running through a class's `__module__` must be collectable.
///
/// `module=` stores its argument unvalidated, so it can be an arbitrary heap
/// object — including one that refers back to the class. This pins the same
/// `for_each_child_id` / `py_dec_ref_ids` invariant for the `module` edge that
/// `namedtuple_class_cycle_is_collected` pins for `defaults`: here `defaults` is
/// empty, so `module` is the only edge holding the cycle together.
#[test]
#[cfg(feature = "test-hooks")]
fn namedtuple_module_cycle_is_collected() {
    let code = r"
import gc
from collections import namedtuple

def build():
    holder = []
    cls = namedtuple('C', ['a'], module=holder)
    holder.append(cls)

build()
gc.collect()
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let freed = run.run_no_limits(vec![]).expect("should run");
    let MontyObject::Int(freed) = freed else {
        panic!("gc.collect() should return an int, got {freed:?}");
    };
    assert!(
        freed > 0,
        "the class -> module -> class cycle should be collected, but gc.collect() freed {freed} entries",
    );
}

/// Runs `code` and returns its final value as a host object.
fn host_value(code: &str) -> MontyObject {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    run.run_no_limits(vec![]).expect("should run")
}

/// A deque must cross the host boundary as a structured list, not a repr string.
///
/// There is no host-side deque *value* type, so it degrades to the nearest
/// structural one — the same choice defaultdict and Counter make in degrading to
/// `dict`. The earlier repr-string fallback was indistinguishable from a genuine
/// `str` on the host and flattened nested values recursively, losing their
/// structure; these assertions pin that it no longer does.
#[test]
fn deque_crosses_host_boundary_as_a_list() {
    assert_eq!(
        host_value("from collections import deque\ndeque([1, 2, 3])"),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2), MontyObject::Int(3)])
    );

    // `maxlen` does not survive the crossing — the host sees only the items.
    assert_eq!(
        host_value("from collections import deque\ndeque([1, 2], maxlen=5)"),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2)])
    );

    // Nested values keep their own types rather than being flattened to text.
    assert_eq!(
        host_value("from collections import deque\n[deque([b'x']), 2]"),
        MontyObject::List(vec![
            MontyObject::List(vec![MontyObject::Bytes(b"x".to_vec())]),
            MontyObject::Int(2)
        ])
    );
}

/// A named tuple subscripted by a string reports as `tuple`, not `namedtuple`.
///
/// CPython raises this from the inherited `tuple.__getitem__`, so the subclass
/// name never appears. This cannot be a dual-run `test_case`: the rest of the
/// message still diverges from CPython's `tuple indices must be integers or
/// slices, not str`, a difference plain tuples and lists share.
#[test]
fn namedtuple_string_subscript_names_tuple() {
    let run = MontyRun::new(
        "from collections import namedtuple\nnamedtuple('P', 'x y')(1, 2)['x']".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .expect("should parse");
    let err = run.run_no_limits(vec![]).expect_err("expected TypeError");
    assert_snapshot!(err.message().expect("TypeError carries a message"), @"tuple indices must be integers, not 'str'");
}
