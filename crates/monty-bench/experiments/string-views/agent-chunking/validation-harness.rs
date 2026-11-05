use std::{env, fs, path::PathBuf};

use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

#[test]
fn document_chunks_match_complete_cpython_output() {
    let directory = PathBuf::from(env::var_os("MONTY_CHUNKING_FIXTURES").unwrap());
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(directory.join("manifest.json")).unwrap()).unwrap();
    for entry in manifest.as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let source = fs::read_to_string(directory.join(format!("{name}.full.py"))).unwrap();
        let data = fs::read_to_string(directory.join(format!("{name}.input"))).unwrap();
        let expected: Vec<(i64, String)> =
            serde_json::from_str(&fs::read_to_string(directory.join(format!("{name}.expected.json"))).unwrap())
                .unwrap();
        let expected = MontyObject::list(
            expected
                .into_iter()
                .map(|(start, text)| MontyObject::tuple([MontyObject::int(start), MontyObject::string(text)])),
        );
        let mut runner = MontyRun::new(
            source,
            "chunking.py",
            vec!["data".to_owned()],
            CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(
            runner.run_no_limits(vec![MontyObject::string(data)]).unwrap(),
            expected,
            "{name}"
        );
    }
}
