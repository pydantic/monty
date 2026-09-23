//! Dump compatibility across builds.
//!
//! The dump codec names every field and variant, so a data-layout change made
//! after a dump was written must still load it. Two tests pin that property:
//! a checked-in dump written by an earlier build of the current
//! `DUMP_VERSION`, and a synthetic old/new type pair round-tripped through the
//! codec itself.

use std::{env, fs, path::PathBuf};

use monty::{DUMP_VERSION, Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};
use serde::{Deserialize, Serialize};

/// Environment variable that rewrites the fixture instead of checking it.
const UPDATE_FIXTURE: &str = "UPDATE_DUMP_FIXTURE";

/// Builds the state the fixture holds: the common heap types, a live
/// iterator, functions, classes and a cycle, ahead of a suspension on a host
/// call whose arguments carry heap values.
const FIXTURE_STATE: &str = r"
import collections
import dataclasses
import datetime
import functools
import re

big = 2**100
neg = -7
f = 3.5
s = 'héllo'
b = b'\x00\x01bytes'
xs = [1, 2.5, 'three', None, True]
t = (1, 'two')
d = {'a': 1, 'b': [2, 3], 3: 'int key'}
st = {1, 2, 3}
fs = frozenset({'x', 'y'})
dq = collections.deque([1, 2, 3], maxlen=5)
Point = collections.namedtuple('Point', ['x', 'y'])
p = Point(1, 2)


@dataclasses.dataclass
class Item:
    name: str
    qty: int = 0


item = Item('widget', 3)


class Counter:
    total = 0

    def __init__(self, start):
        self.value = start

    def bump(self):
        self.value += 1
        return self.value


c = Counter(10)
c.bump()


def make_adder(n):
    def add(x):
        return x + n

    return add


add5 = make_adder(5)


it = iter([1, 2])
next(it)

try:
    raise ValueError('boom')
except ValueError as exc:
    err = exc

dt = datetime.datetime(2024, 1, 2, 3, 4, 5, 678901)
dlt = datetime.timedelta(days=1, seconds=2)
pat = re.compile(r'(\d+)-(\w+)')
m = pat.match('12-ab')
part = functools.partial(add5, 1)
cycle = []
cycle.append(cycle)
";

/// The host call the fixture is suspended on.
const FIXTURE_CALL: &str = "host_call(xs, d, b) + 1";

/// Fed after restoring and resuming the fixture: every value must have survived.
const FIXTURE_CHECK: &str = r"
assert big == 2**100 and neg == -7 and f == 3.5 and s == 'héllo'
assert b == b'\x00\x01bytes'
assert xs == [1, 2.5, 'three', None, True] and t == (1, 'two')
assert d == {'a': 1, 'b': [2, 3], 3: 'int key'}
assert st == {1, 2, 3} and fs == frozenset({'x', 'y'})
assert list(dq) == [1, 2, 3] and dq.maxlen == 5
assert p == Point(1, 2) and p.x == 1
assert item == Item('widget', 3)
assert c.value == 11 and c.bump() == 12 and Counter.total == 0
assert add5(1) == 6 and part() == 6
assert next(it) == 2
assert str(err) == 'boom' and isinstance(err, ValueError)
assert dt.microsecond == 678901 and dlt.days == 1
assert m.groups() == ('12', 'ab') and pat.sub('X', '1-a 2-b') == 'X X'
assert cycle[0] is cycle
(result, c.value)
";

/// A dump written by an earlier build at this `DUMP_VERSION` still loads and
/// runs. Failing here means a data-layout change broke older dumps: add a
/// `#[serde(alias)]` or `#[serde(default)]`, or bump `DUMP_VERSION` and
/// regenerate with `UPDATE_DUMP_FIXTURE=1 cargo test -p monty --test dump_compat`.
#[test]
fn fixture_dump_loads_and_resumes() {
    let path = fixture_path();
    if env::var_os(UPDATE_FIXTURE).is_some() {
        let mut repl = MontyRepl::new("fixture.py", ResourceTracker::default(), CompileOptions::default());
        repl.feed_run(FIXTURE_STATE, vec![], PrintWriter::Stdout).unwrap();
        let progress = repl.feed_start(FIXTURE_CALL, vec![], PrintWriter::Stdout).unwrap();
        let bytes = dump("fixture.py", None, SessionRef::Suspended(&progress)).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        eprintln!("updated {}", path.display());
        return;
    }
    let bytes = fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "failed to read {}: {err}; regenerate with {UPDATE_FIXTURE}=1 cargo test -p monty --test dump_compat",
            path.display()
        )
    });
    let loaded = Dump::load(&bytes).expect("a fixture at the current DUMP_VERSION loads");
    assert_eq!(loaded.script_name, "fixture.py");
    assert!(loaded.type_check.is_none());
    let Session::Suspended(progress) = loaded.state else {
        panic!("fixture was dumped suspended on a host call");
    };
    let call = progress.into_function_call().expect("suspended on host_call");
    assert_eq!(call.function_name, "host_call");
    // the call arguments are a value graph of their own inside the dump
    assert_eq!(
        call.args.args().collect::<Vec<_>>(),
        vec![
            MontyObject::list([
                MontyObject::int(1),
                MontyObject::float(2.5),
                MontyObject::string("three"),
                MontyObject::none(),
                MontyObject::bool(true),
            ]),
            MontyObject::dict([
                (MontyObject::string("a"), MontyObject::int(1)),
                (
                    MontyObject::string("b"),
                    MontyObject::list([MontyObject::int(2), MontyObject::int(3)]),
                ),
                (MontyObject::int(3), MontyObject::string("int key")),
            ]),
            MontyObject::bytes(b"\x00\x01bytes".as_slice()),
        ]
    );
    let progress = call.resume(MontyObject::int(41), PrintWriter::Stdout).unwrap();
    let (mut repl, value) = progress.into_complete().expect("host_call resumed to completion");
    assert_eq!(value, MontyObject::int(42));
    repl.feed_run("result = 42", vec![], PrintWriter::Stdout).unwrap();
    let checked = repl.feed_run(FIXTURE_CHECK, vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(
        checked,
        MontyObject::tuple(vec![MontyObject::int(42), MontyObject::int(12)])
    );
}

/// Dict and set entries carry no hash in a dump; each container rebuilds its
/// index on the first keyed operation after loading. Every kind of keyed
/// access is driven here on loaded containers, including keys whose `__hash__`
/// is user code, which runs again exactly once per key.
#[test]
fn loaded_dicts_and_sets_rebuild_their_indices_lazily() {
    let mut repl = MontyRepl::new("lazy.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(LAZY_STATE, vec![], PrintWriter::Stdout).unwrap();
    let bytes = dump("lazy.py", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped an idle session");
    };
    let checked = loaded.feed_run(LAZY_CHECK, vec![], PrintWriter::Stdout).unwrap();
    assert_eq!(checked, MontyObject::bool(true));
}

/// Containers keyed by everything that hashes differently: immediates, strings,
/// tuples, frozensets and instances with a counting `__hash__`.
const LAZY_STATE: &str = r"
import collections

hash_calls = []


class Key:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        hash_calls.append(self.n)
        return self.n

    def __eq__(self, other):
        return isinstance(other, Key) and other.n == self.n


class Point:
    origin = (0, 0)

    def __init__(self, x):
        self.x = x


k1, k2 = Key(1), Key(2)
d = {(1, 2): 'tuple', frozenset({3}): 'frozen', k1: 'key', None: 'none', int: 'type', 'str': 1}
s = {(1, 2), frozenset({3}), k2, None, int, 'str'}
fs = frozenset(s)
c = collections.Counter(a=3, b=1)
dd = collections.defaultdict(list)
dd['x'].append(1)
p = Point(5)
hash_calls.clear()
";

/// The rebuilds run each stored `__hash__` once, then every keyed path on the
/// loaded containers.
const LAZY_CHECK: &str = r"
import collections, math

assert hash_calls == []
assert d[None] == 'none'
assert hash_calls == [1]
assert None in s
assert hash_calls == [1, 2]
assert d[None] == 'none' and None in s
assert hash_calls == [1, 2]
assert d[(1, 2)] == 'tuple' and d[frozenset({3})] == 'frozen' and d[k1] == 'key'
assert d[None] == 'none' and d[int] == 'type' and d['str'] == 1
assert (1, 2) in d and Key(1) in d and Key(3) not in d
assert d == {(1, 2): 'tuple', frozenset({3}): 'frozen', Key(1): 'key', None: 'none', int: 'type', 'str': 1}
assert d.keys() == {(1, 2), frozenset({3}), Key(1), None, int, 'str'}
assert d.pop(None) == 'none' and d.popitem() == ('str', 1) and d.setdefault('new', 2) == 2
assert (1, 2) in s and Key(2) in s and Key(3) not in s and 'str' in s
assert s == fs and s | {7} == fs | {7} and s & {(1, 2)} == {(1, 2)} and s - fs == set()
assert s.issubset(fs) and s.issuperset(fs) and s.isdisjoint({8})
assert len({fs: 1}) == 1 and hash(fs) == hash(frozenset(s))
s.discard(None)
assert s.pop() is not None and len(s) == 4
assert c['a'] == 3 and c['missing'] == 0 and c == collections.Counter(a=3, b=1)
assert c <= collections.Counter(a=3, b=1, c=1) and (c + c)['a'] == 6 and (c - c)['a'] == 0
assert dd['x'] == [1] and dd['y'] == []
assert p.x == 5 and Point.origin == (0, 0) and math.sqrt(4) == 2
p.y = 6
assert p.y == 6 and p.x == 5
True
";

/// The fixture for the current `DUMP_VERSION`; the name carries the version so
/// a bump leaves the old file behind to delete rather than silently reuse.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("dump_v{DUMP_VERSION}.bin"))
}

/// A record as an older build wrote it.
#[derive(Serialize)]
struct RecordV1 {
    id: u32,
    name: String,
    kind: KindV1,
    retired: bool,
}

#[derive(Serialize)]
enum KindV1 {
    Alpha,
    Beta(u8),
}

/// The same record after every kind of layout edit: a field inserted before
/// the others, one renamed, one removed, one appended, and a variant inserted
/// mid-enum.
#[derive(Debug, PartialEq, Deserialize)]
struct RecordV2 {
    #[serde(default)]
    inserted: Option<String>,
    id: u32,
    #[serde(alias = "name")]
    label: String,
    kind: KindV2,
    #[serde(default)]
    appended: u64,
}

#[derive(Debug, PartialEq, Deserialize)]
enum KindV2 {
    Alpha,
    Gamma,
    Beta(u8),
}

/// The codec property the fixture test relies on, asserted here rather than
/// assumed from upstream: an older layout loads into the edited one.
#[test]
fn layout_edits_load_older_encodings() {
    for (old, expected) in [
        (
            RecordV1 {
                id: 1,
                name: "one".to_owned(),
                kind: KindV1::Alpha,
                retired: true,
            },
            RecordV2 {
                inserted: None,
                id: 1,
                label: "one".to_owned(),
                kind: KindV2::Alpha,
                appended: 0,
            },
        ),
        (
            RecordV1 {
                id: 2,
                name: "two".to_owned(),
                kind: KindV1::Beta(7),
                retired: false,
            },
            RecordV2 {
                inserted: None,
                id: 2,
                label: "two".to_owned(),
                kind: KindV2::Beta(7),
                appended: 0,
            },
        ),
    ] {
        let bytes = minicbor_serde::to_vec(&old).unwrap();
        assert_eq!(minicbor_serde::from_slice::<RecordV2>(&bytes).unwrap(), expected);
    }
}
