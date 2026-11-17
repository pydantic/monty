//! Subscription ownership, suspension, and execution limits.

use std::{fmt::Write, time::Duration};

#[cfg(not(feature = "memory-model-checks"))]
use monty::{Dump, Session, SessionRef, dump};
use monty::{MontyRepl, MontyRun, ReplProgress};
use monty_types::{
    CompileOptions, ExcType, MontyObject, NameLookupResult, PrintWriter, ResourceLimits, ResourceTracker,
};

fn round_trip(progress: ReplProgress) -> ReplProgress {
    // Discarding the original suspended state is unsupported by the ownership-checking build.
    #[cfg(feature = "memory-model-checks")]
    {
        progress
    }
    #[cfg(not(feature = "memory-model-checks"))]
    {
        let bytes = dump("lookup.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
        drop(progress);
        let Session::Suspended(restored) = Dump::load(&bytes).unwrap().state else {
            panic!("expected a suspended REPL");
        };
        *restored
    }
}

#[test]
fn subscript_name_lookup_resumes_after_dump() {
    for (source, names) in [
        ("source[key]", vec!["source", "key"]),
        ("def run():\n    return source[key]\nrun()", vec!["source", "key"]),
        ("def run(source):\n    return source[key]\nrun([41])", vec!["key"]),
        ("def run(key):\n    return source[key]\nrun(0)", vec!["source"]),
    ] {
        let repl = MontyRepl::new("lookup.py", ResourceTracker::default(), CompileOptions::default());
        let mut progress = repl.feed_start(source, vec![], PrintWriter::Stdout).unwrap();

        for name in names {
            // The second lookup suspends with the evaluated container already on the operand stack.
            let restored = round_trip(progress);
            let lookup = restored
                .into_name_lookup()
                .expect("expected unresolved subscript operand");
            assert_eq!(lookup.name, name);

            let value = if name == "source" {
                MontyObject::list([MontyObject::int(41)])
            } else {
                MontyObject::int(0)
            };
            progress = lookup
                .resume(NameLookupResult::Value(value), PrintWriter::Stdout)
                .unwrap();
        }

        let (_, value) = progress.into_complete().expect("subscript should complete");
        assert_eq!(value, MontyObject::int(41));
    }
}

#[test]
fn compiled_subscripts_survive_dump_and_reuse() {
    let mut repl = MontyRepl::new("lookup.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "def lookup(source, key):\n    result = source[key]\n    return result\ndef fields(rows):\n    return [row['value'] for row in rows]",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let progress = repl.feed_start("gate()", vec![], PrintWriter::Stdout).unwrap();
    let (mut repl, _) = round_trip(progress)
        .into_function_call()
        .unwrap()
        .resume(MontyObject::none(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    let value = repl
        .feed_run(
            "assert lookup([41], 0) == 41\nassert lookup({'x': [42]}, 'x') == [42]\nfields([{'value': 43}])",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(value, MontyObject::list([MontyObject::int(43)]));
}

#[test]
#[cfg(not(feature = "memory-model-checks"))]
fn original_opcode_dump_still_resumes() {
    let loaded = Dump::load(include_bytes!("fixtures/dump_v15.bin")).unwrap();
    let Session::Suspended(progress) = loaded.state else {
        panic!("expected the version-15 host call");
    };
    let progress = progress
        .into_function_call()
        .unwrap()
        .resume(MontyObject::int(41), PrintWriter::Stdout)
        .unwrap();
    let (_, value) = progress.into_complete().unwrap();
    assert_eq!(value, MontyObject::int(42));
}

#[test]
fn native_subscript_loops_observe_time_limit() {
    for (setup, access) in [
        ("values = [1]\nkey = 0", "values[key]"),
        ("values = {'key': 1}\nkey = 'key'", "values[key]"),
        ("values = [1]", "values[0]"),
        ("values = {'key': 1}", "values['key']"),
        ("global values\nvalues = [1]\nkey = 0", "values[key]"),
        ("global values\nvalues = {'key': 1}", "values['key']"),
    ] {
        let source = format!(
            "def run():\n{}\n    while True:\n        result = {access}\nrun()",
            setup
                .lines()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut runner = MontyRun::new(source, "lookup.py", vec![], CompileOptions::default()).unwrap();
        let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(10));
        let error = runner
            .run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
            .expect_err("native hits must still reach dispatch checkpoints");
        assert_eq!(error.exc_type(), ExcType::TimeoutError);
    }
}

#[test]
fn fused_subscripts_cross_dispatch_checkpoints_and_reentry() {
    let mut source = String::from("global_values = [0]\ndef check(values, key):\n");
    // Changing results expose a skipped store; repeated groups exercise both
    // ordinary dispatch batches and the shorter chains used during __repr__.
    for number in 1000..1260 {
        writeln!(
            source,
            "    values[0] = {number}\n    global_values[0] = {number}\n    result = values[key]\n    literal = values[0]\n    external = global_values[0]\n    assert result == literal == external == {number}"
        )
        .unwrap();
    }
    source.push_str(
        "    return result\nclass Probe:\n    def __repr__(self):\n        assert check([0], 0) == 1259\n        return 'checked'\nassert check([0], 0) == 1259\nassert repr(Probe()) == 'checked'\n",
    );
    let mut runner = MontyRun::new(source, "lookup.py", vec![], CompileOptions::default()).unwrap();
    runner
        .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
}

#[test]
fn global_and_literal_subscripts_replace_aliased_and_wide_destinations() {
    let mut source = String::from("values = {'key': 1000}\nkey = 'key'\noutput = []\ndef run():\n    global output\n");
    for slot in 0..300 {
        writeln!(source, "    unused_{slot} = None").unwrap();
    }
    source.push_str(
        r"
    wide_key = 'key'
    local_values = {'key': 1001}
    for _ in range(300):
        result = []
        result = values[wide_key]
        assert result == 1000
        output = []
        output = values[key]
        assert output == 1000
        output = local_values['key']
        assert output == 1001
        local_values = [local_values]
        local_values = local_values[0]
        assert local_values['key'] == 1001
    return values['key']
assert run() == 1000
values = [1002]
values = values[0]
assert values == 1002
",
    );
    let mut runner = MontyRun::new(source, "lookup.py", vec![], CompileOptions::default()).unwrap();
    runner
        .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
}

#[test]
#[cfg(feature = "ref-count-return")]
fn subscript_assignment_releases_replaced_containers() {
    let source = r"
def run():
    global output
    for repeat in range(1000):
        item = [repeat]
        values = [item]
        index = 0
        values = values[index]
        index = values[index]
        mapping = {'key': values}
        key = 'key'
        mapping = mapping[key]
        assert mapping[0] == index

        cycle = []
        cycle.append(cycle)
        values = [cycle, [repeat]]
        key = 0
        result = values[key]
        result = values[key]
        key = 1
        result = values[key]
        values = values[key]
        key = 0
        result = values[key]
        output = cycle
        cycle = None
        output = values[key]
        assert result == output == repeat
    return 0
run()
";
    let mut runner = MontyRun::new(source.to_owned(), "lookup.py", vec![], CompileOptions::default()).unwrap();
    let output = runner.run_ref_counts(vec![]).unwrap();
    assert_eq!(output.unreachable, Vec::<String>::new());
}

#[test]
fn local_subscripts_cross_encoding_boundaries() {
    for (object, key) in [(127, 128), (128, 129), (254, 255), (255, 254), (256, 257), (0, 256)] {
        let mut source = String::from("def read():\n");
        for slot in 0..=object.max(key) {
            if slot == object {
                source.push_str("    values = {0: 42, 'key': 43}\n");
            } else if slot == key {
                source.push_str("    key = 0\n");
            } else {
                writeln!(source, "    unused_{slot} = None").unwrap();
            }
        }
        source.push_str("    return values[0] + values['key'] + values[key]\nread()\n");
        let mut runner = MontyRun::new(source, "local_bounds.py", vec![], CompileOptions::default()).unwrap();
        assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::int(127));
    }
}

#[test]
fn global_subscripts_cross_packed_slot_boundary() {
    for slot in [8191, 8192] {
        let mut source = String::new();
        for unused in 0..slot {
            writeln!(source, "unused_{unused} = None").unwrap();
        }
        source.push_str("values = {0: 42, 'key': 43}\nkey = 0\nvalues[0] + values['key'] + values[key]\n");
        let mut runner = MontyRun::new(source, "global_bounds.py", vec![], CompileOptions::default()).unwrap();
        assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::int(127));
    }
}

#[test]
fn subscript_integer_literals_cross_operand_widths() {
    let mut source = String::from(
        "values = {-32769: 1, -32768: 2, -129: 3, -128: 4, 127: 5, 128: 6, 32767: 7, 32768: 8}\ndef read(local):\n",
    );
    for (index, key) in [-32769, -32768, -129, -128, 127, 128, 32767, 32768]
        .into_iter()
        .enumerate()
    {
        writeln!(
            source,
            "    assert local[{key}] == {}\n    assert values[{key}] == {}",
            index + 1,
            index + 1
        )
        .unwrap();
    }
    source.push_str("read(values)\n");
    let mut runner = MontyRun::new(source, "literal_bounds.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}
