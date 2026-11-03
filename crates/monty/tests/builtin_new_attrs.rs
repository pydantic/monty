use monty::{Dump, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, MontyType, PrintWriter, ResourceTracker};

#[test]
fn new_builtin_tables_and_string_representations() {
    let code = include_str!("../test_cases/builtin__new_type_attrs.py");
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

#[test]
fn table_forward_ref_evaluation_can_suspend_and_restore() {
    let run = MontyRun::new(
        "from annotationlib import ForwardRef\nForwardRef.evaluate(ForwardRef('f()'), globals={'f': f})".to_owned(),
        "test.py",
        vec!["f".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let progress = run
        .start(
            vec![MontyObject::function("fetch", None)],
            ResourceTracker::default(),
            PrintWriter::Disabled,
        )
        .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
    let Session::Running(restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected a running session")
    };
    for progress in [progress, *restored] {
        let call = progress.into_function_call().expect("expected the fetch call");
        assert_eq!(call.function_name, "fetch");
        let result = call
            .resume(MontyObject::type_object(MontyType::Int), PrintWriter::Disabled)
            .unwrap()
            .into_complete()
            .unwrap();
        assert_eq!(result, MontyObject::type_object(MontyType::Int));
    }
}
