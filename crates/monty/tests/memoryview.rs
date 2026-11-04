use std::time::Duration;

use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

#[test]
fn memoryview_core() {
    let mut runner = MontyRun::new(
        include_str!("../test_cases/memoryview__core.py").to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn memoryview_snapshot_preserves_aliases_and_exports() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "a = bytearray(b'abc')\nv = memoryview(a)\nw = v[::-1]\nv.release()\nit = iter(w)\nassert next(it) == 99",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut repl) = Dump::load(&bytes).unwrap().state else {
        panic!("idle dump");
    };
    repl.feed_run("w[0] = 90\nassert a == b'abZ'\nassert list(it) == [98, 97]\ntry:\n    a.append(1)\nexcept BufferError:\n    pass\nelse:\n    assert False\nw = None\na.append(1)\nassert a == b'abZ\\x01'", vec![], PrintWriter::Stdout).unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn memoryview_cycle_collection_releases_exports() {
    let mut runner = MontyRun::new("import gc\na = bytearray(b'abc')\nv = memoryview(a)\nc = [v]\nc.append(c)\nv = None\nc = None\ngc.collect()\na.append(1)\nassert a == b'abc\\x01'\nfor i in range(20):\n    a = bytearray(b'abc')\n    c = [a, memoryview(a)]\n    c.append(c)\n    a = None\n    c = None\n    gc.collect()".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn memoryview_snapshot_release_states_and_cached_hash() {
    for releases in ["", "v.release()", "w.release()", "v.release()\nw.release()"] {
        let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
        let source = format!(
            "a = bytearray(b'abc')\nv = memoryview(a)\nw = v[::-1]\nr = w.toreadonly()\nh = memoryview(b'abc')\nexpected_hash = hash(h)\nh.release()\n{releases}"
        );
        repl.feed_run(&source, vec![], PrintWriter::Stdout).unwrap();
        for _ in 0..3 {
            let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
            let Session::Idle(restored) = Dump::load(&bytes).unwrap().state else {
                panic!("idle dump");
            };
            repl = *restored;
        }
        repl.feed_run("assert hash(h) == expected_hash\nassert r.obj is a\na[0] = 90\nassert r.tobytes() == b'cbZ'\nv.release()\nw.release()\nr.release()\na.append(1)\nassert a == b'Zbc\\x01'", vec![], PrintWriter::Stdout).unwrap();
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn memoryview_cycle_collection_preserves_live_sibling() {
    let mut runner = MontyRun::new("import gc\nfor i in range(100):\n    owner = bytearray(b'abcd')\n    live = memoryview(owner)\n    garbage = [memoryview(live)[::2], live.toreadonly()]\n    garbage.append(garbage)\n    garbage = None\n    gc.collect()\n    try:\n        owner.append(0)\n    except BufferError:\n        pass\n    else:\n        assert False\n    live = None\n    owner.append(0)\n    assert owner == b'abcd\\0'".to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn memoryview_snapshot_preserves_pending_frame_buffers() {
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl.feed_start("def run():\n    owner = bytearray(b'abc')\n    view = memoryview(owner)\n    host_call()\n    view[0] = 90\n    view.release()\n    owner.append(0)\n    return bytes(owner)\nrun()", vec![], PrintWriter::Stdout).unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    drop(progress.into_repl());
    let Session::Suspended(restored) = Dump::load(&bytes).unwrap().state else {
        panic!("suspended dump");
    };
    let call = restored.into_function_call().expect("paused at host call");
    let progress = call.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
    let (_, value) = progress.into_complete().expect("completed after host call");
    assert_eq!(value, MontyObject::bytes(b"Zbc\0".as_slice()));
}

#[cfg(feature = "test-hooks")]
#[test]
fn memoryview_snapshot_pending_cycle_preserves_live_export() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("import gc\na = bytearray(b'abc')\nv = memoryview(a)\ngarbage = [memoryview(a)]\ngarbage.append(garbage)\ngarbage = None", vec![], PrintWriter::Stdout).unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("idle dump");
    };
    restored.feed_run("gc.collect()\ntry:\n    a.append(0)\nexcept BufferError:\n    pass\nelse:\n    assert False\nv.release()\na.append(0)\nassert a == b'abc\\0'", vec![], PrintWriter::Stdout).unwrap();
}

#[test]
fn memoryview_multidimensional_snapshot_preserves_layout_and_exports() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("owner = bytearray(32)\nv = memoryview(owner).cast('I', [2, 4])\nv[1, 2] = 123456\nr = v[::-1].toreadonly()\nv.release()\nzero = memoryview(b'\\x01\\x00\\x00\\x00').cast('I', [])", vec![], PrintWriter::Stdout).unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("idle dump");
    };
    restored.feed_run("assert r.shape == (2, 4) and r.strides == (-16, 4)\nassert r[0, 2] == 123456\nassert r.tolist() == [[0, 0, 123456, 0], [0, 0, 0, 0]]\nassert zero.shape == () and zero[()] == 1\ntry:\n    owner.append(0)\nexcept BufferError:\n    pass\nelse:\n    assert False\nr.release()\nowner.append(0)\nassert len(owner) == 33", vec![], PrintWriter::Stdout).unwrap();
}

#[test]
fn memoryview_max_dimensions_and_recursion_limit() {
    let code = "v = memoryview(b'\\0').cast('B', [1] * 64)\nassert v.ndim == 64\nvalue = v.tolist()\nfor i in range(64):\n    value = value[0]\nassert value == 0";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
    let error = runner
        .run(
            vec![],
            ResourceTracker::new(ResourceLimits::default().max_recursion_depth(8)),
            PrintWriter::Stdout,
        )
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::RecursionError);
}

#[test]
fn memoryview_multidimensional_native_loops_honor_time_limit() {
    for operation in ["view.tobytes(order='F')", "view.tolist()"] {
        let code = format!(
            "shape = [1] * 64\nshape[0] = 1000\nshape[-1] = 1000\nview = memoryview(bytearray(1_000_000)).cast('B', shape)[::2]\n{operation}"
        );
        let mut runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
        let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(10));
        let error = runner
            .run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
            .unwrap_err();
        assert_eq!(error.exc_type(), ExcType::TimeoutError, "{operation}");
    }
}

#[test]
fn memoryview_host_export_uses_sandbox_repr() {
    for source in ["memoryview(b'abc')", "view = memoryview(b'abc')\nview.release()\nview"] {
        let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        let result = runner.run_no_limits(vec![]).unwrap();
        let expected = if source.starts_with("view =") {
            "<released memory at 0x0>"
        } else {
            "<memory at 0x0>"
        };
        assert_eq!(result, MontyObject::repr(expected));
    }
}

#[test]
fn memoryview_slice_assignment_stops_before_finishing_on_timeout() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "owner = bytearray(16 * 1024 * 1024)\nview = memoryview(owner)\nrhs = b'x' * (8 * 1024 * 1024)",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    repl.tracker_mut().set_max_feed_duration(Duration::from_millis(10));
    let error = repl
        .feed_run("view[::2] = rhs", vec![], PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
    repl.tracker_mut().set_max_feed_duration(Duration::from_secs(10));
    repl.feed_run(
        "assert owner[-2] == 0\nview.release()\nowner.append(1)\nassert owner[-1] == 1",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn memoryview_index_callback_release_is_safe_during_collection() {
    let source = r"
import gc

class Coordinate:
    def __index__(self):
        view.release()
        owner.clear()
        gc.collect()
        return 0

for code in 'Bbc?hHiIlLqQnNefdP':
    size = {'B':1, 'b':1, 'c':1, '?':1, 'h':2, 'H':2, 'e':2, 'i':4, 'I':4, 'f':4}.get(code, 8)
    for prefix in ['', '@']:
        for multidimensional in [False, True]:
            for write in [False, True]:
                owner = bytearray(8 * size)
                shape = [2, 2, 2] if multidimensional else [8]
                view = memoryview(owner).cast(prefix + code, shape)
                key = (Coordinate(), 0, 0) if multidimensional else Coordinate()
                try:
                    if write:
                        view[key] = 1
                    else:
                        view[key]
                except ValueError as exc:
                    assert str(exc) == 'operation forbidden on released memoryview object'
                else:
                    assert False
                assert owner == b''
                owner.append(1)
                gc.collect()
";
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn memoryview_snapshot_after_scalar_callbacks() {
    for (format, method, result) in [("Q", "__index__", "18446744073709551615"), ("d", "__float__", "1.5")] {
        let code = format!(
            r"owner = bytearray(32)
parent = memoryview(owner).cast('{format}', [2, 2])
view = parent[::-1]
parent.release()
readonly = view.toreadonly()
class Scalar:
    def {method}(self):
        return {result}
view[0, 1] = Scalar()
host_call()
assert readonly[0, 1] == {result}
view.release()
try:
    owner.clear()
except BufferError:
    pass
else:
    assert False
readonly.release()
owner.clear()
len(owner)"
        );
        let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
        let progress = repl.feed_start(&code, vec![], PrintWriter::Stdout).unwrap();
        let bytes = dump("test.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
        drop(progress.into_repl());
        let Session::Suspended(restored) = Dump::load(&bytes).unwrap().state else {
            panic!("suspended dump");
        };
        let call = restored.into_function_call().expect("paused after scalar conversion");
        let progress = call.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
        let (_, value) = progress.into_complete().expect("completed scalar assignment");
        assert_eq!(value, MontyObject::int(0));
    }
}

#[test]
fn memoryview_snapshot_after_coordinate_callback_with_iterator() {
    let code = r"owner = bytearray(32)
flat = memoryview(owner).cast('Q')
iterator = iter(flat)
assert next(iterator) == 0
matrix = flat.cast('B').cast('Q', [2, 2])
class Coordinate:
    def __index__(self):
        return 1
matrix[Coordinate(), 0] = 123
host_call()
assert next(iterator) == 0
assert next(iterator) == 123
assert next(iterator) == 0
matrix.release()
flat.release()
iterator = None
owner.clear()
len(owner)";
    let repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let progress = repl.feed_start(code, vec![], PrintWriter::Stdout).unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    drop(progress.into_repl());
    let Session::Suspended(restored) = Dump::load(&bytes).unwrap().state else {
        panic!("suspended dump");
    };
    let call = restored
        .into_function_call()
        .expect("paused after coordinate conversion");
    let progress = call.resume(MontyObject::none(), PrintWriter::Stdout).unwrap();
    let (_, value) = progress.into_complete().expect("completed indexed assignment");
    assert_eq!(value, MontyObject::int(0));
}

#[cfg(feature = "test-hooks")]
#[test]
fn memoryview_search_callbacks_release_and_collect() {
    let code = r"import gc
owner = None
view = None
answer = False
class Needle:
    def __eq__(self, other):
        view.release()
        owner.clear()
        gc.collect()
        return answer
for format in ['B', 'c', 'Q', 'd']:
    for answer in [False, True]:
        for operation in ['count', 'index', 'contains']:
            owner = bytearray(32)
            view = memoryview(owner).cast(format)
            try:
                if operation == 'count':
                    view.count(Needle())
                elif operation == 'index':
                    assert view.index(Needle()) == 0
                else:
                    assert Needle() in view
            except ValueError as exc:
                assert operation == 'count' or not answer
                assert str(exc) == 'operation forbidden on released memoryview object'
            else:
                assert answer and operation != 'count'
            owner.append(1)
            assert owner == b'\x01'";
    let mut runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn memoryview_released_iterator_snapshot_preserves_exhaustion() {
    for (format, itemsize) in [("B", 1), ("c", 1), ("Q", 8), ("d", 8)] {
        for length in 0..=2 {
            for consumed in 0..=length {
                let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
                repl.feed_run(
                    &format!(
                        "owner = bytearray({})\nview = memoryview(owner).cast('{format}')\nit = iter(view)\nfor _ in range({consumed}):\n    next(it)\nview.release()\nowner.clear()",
                        length * itemsize
                    ),
                    vec![],
                    PrintWriter::Stdout,
                )
                .unwrap();
                let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
                drop(repl);
                let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
                    panic!("idle dump");
                };
                restored
                    .feed_run(
                        &format!(
                            "for _ in range(2):\n    try:\n        next(it)\n    except StopIteration:\n        assert {consumed} == {length}\n    except ValueError as exc:\n        assert {consumed} < {length}\n        assert str(exc) == 'operation forbidden on released memoryview object'\n    else:\n        assert False\nowner.append(1)\nassert owner == b'\\x01'"
                        ),
                        vec![],
                        PrintWriter::Stdout,
                    )
                    .unwrap();
            }
        }
    }
}
