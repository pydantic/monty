use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

#[test]
fn table_methods_reject_invalid_arguments_and_remain_usable() {
    let imports = "from datetime import date, time, timedelta\nimport re\n";
    for expression in [
        "[].reverse([1])",
        "[].clear([1])",
        "[].copy([1])",
        "date(2024, 1, 2).weekday([1])",
        "date(2024, 1, 2).isoweekday([1])",
        "timedelta(seconds=1).total_seconds([1])",
        "re.search('(a)', 'a').groups([1])",
        "re.compile('a').search()",
        "re.compile('a').match()",
        "re.compile('a').fullmatch()",
        "re.compile('a').findall()",
        "re.compile('a').finditer()",
        "re.compile('a').search([1])",
        "re.compile('a').match([1])",
        "re.compile('a').fullmatch([1])",
        "re.compile('a').findall([1])",
        "re.compile('a').finditer([1])",
    ] {
        let code = format!(
            "{imports}try:\n    {expression}\nexcept TypeError:\n    pass\nelse:\n    raise AssertionError('expected TypeError')\nassert re.compile('a').search('a').group() == 'a'\nTrue"
        );
        let mut run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
        assert_eq!(
            run.run_no_limits(vec![]).unwrap(),
            MontyObject::bool(true),
            "{expression}"
        );
    }
}

#[test]
fn table_methods_propagate_value_errors() {
    for expression in [
        "date(2024, 1, 31).replace(month=2)",
        "date(2024, 1, 2).replace(year=0)",
        "time(1, 2, 3).isoformat(timespec='invalid')",
    ] {
        let code = format!(
            "from datetime import date, time\ntry:\n    {expression}\nexcept ValueError:\n    pass\nelse:\n    raise AssertionError('expected ValueError')\nTrue"
        );
        let mut run = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
        assert_eq!(
            run.run_no_limits(vec![]).unwrap(),
            MontyObject::bool(true),
            "{expression}"
        );
    }
}
