use monty::MontyRun;
use monty_types::CompileOptions;

#[test]
fn list_slice_assignment() {
    let source = include_str!("../test_cases/list__slice_assignment.py");
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}
