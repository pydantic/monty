use monty::MontyRun;
use monty_types::CompileOptions;

#[test]
fn builtin_containers_mutate_through_aliases() {
    let source = include_str!("../test_cases/inplace__containers.py");
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn set_mutation_keeps_snapshotted_members_alive_during_callbacks() {
    let source = r"
import gc

armed = False

class Key:
    def __init__(self, n):
        self.n = n
        self.cycle = self

    def __hash__(self):
        return 0

    def __eq__(self, other):
        if armed:
            source.clear()
            gc.collect()
        return self.n == other.n

target = {Key(1)}
source = {Key(2), Key(3)}
alias = target
armed = True
OPERATION
assert target is alias
assert len(target) == EXPECTED
";
    for (operation, expected) in [("|=", 3), ("&=", 0), ("-=", 1), ("^=", 3)] {
        let source = source
            .replace("OPERATION", &format!("target {operation} source"))
            .replace("EXPECTED", &expected.to_string());
        let mut runner = MontyRun::new(source, "test.py", vec![], CompileOptions::default()).unwrap();
        runner.run_no_limits(vec![]).unwrap();
    }
}
