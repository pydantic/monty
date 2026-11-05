use std::{env, fs, hint::black_box, path::PathBuf};

use monty::{MontyRepl, MontyRun};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

#[test]
#[ignore = "one fresh process per scenario for peak RSS"]
fn measure_peak_rss() {
    let name = env::var("MONTY_RSS_CASE").unwrap();
    if name == "empty" {
        return;
    }
    let directory = PathBuf::from(env::var_os("MONTY_RSS_FIXTURES").unwrap());
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(directory.join("manifest.json")).unwrap()).unwrap();
    let entry = manifest.as_array().unwrap().iter().find(|v| v["name"] == name).unwrap();
    let source = fs::read_to_string(directory.join(format!("{name}.py"))).unwrap();
    let data = fs::read_to_string(directory.join(format!("{name}.input"))).unwrap();
    let expected = MontyObject::int(entry["expected"].as_i64().unwrap());
    let mut runner = MontyRun::new(
        source.clone(),
        "scenario.py",
        vec!["data".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let value = MontyObject::string(data);
    if entry["retain"].as_bool().unwrap() {
        let mut repl = MontyRepl::new("scenario.py", ResourceTracker::default(), CompileOptions::default());
        let result = repl
            .feed_run(&source, vec![("data".to_owned(), value.clone())], PrintWriter::Disabled)
            .unwrap();
        assert_eq!(result, expected);
        black_box(repl);
    } else {
        assert_eq!(runner.run_no_limits(vec![value.clone()]).unwrap(), expected);
    }
    black_box(value);
    black_box(runner);
}
