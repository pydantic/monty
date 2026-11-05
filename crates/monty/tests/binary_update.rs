use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, MontyException, MontyFileHandle, MontyObject, OsFunctionCall, PrintWriter, ResourceTracker,
};

fn run(code: &str, snapshot: bool) {
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let mut progress = runner
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap();
    let mut content = b"old contents".to_vec();
    loop {
        if snapshot && matches!(progress, RunProgress::OsCall(_)) {
            let bytes = dump("test.py", None, &[], SessionRef::Running(&progress)).unwrap();
            let Session::Running(loaded) = Dump::load(&bytes).unwrap().state else {
                panic!("wrong session kind")
            };
            let original = progress.into_os_call().unwrap();
            assert!(
                original
                    .abort(
                        MontyException::new(ExcType::RuntimeError, Some("discard original".to_owned())),
                        PrintWriter::Stdout
                    )
                    .is_err()
            );
            progress = *loaded;
        }
        progress = match progress {
            RunProgress::Complete(_) => break,
            RunProgress::OsCall(call) => {
                let result = match &call.function_call {
                    OsFunctionCall::Open(args) => {
                        assert!(args.mode.truncate());
                        content.clear();
                        MontyObject::file_handle(MontyFileHandle {
                            path: args.path.to_string(),
                            mode: args.mode,
                            position: 0,
                        })
                    }
                    OsFunctionCall::WriteBytesAt(args) => {
                        args.validate().unwrap();
                        if !args.data.is_empty() {
                            let offset = usize::try_from(args.offset).unwrap();
                            let end = offset + args.data.len();
                            content.resize(content.len().max(end), 0);
                            content[offset..end].copy_from_slice(&args.data);
                        }
                        MontyObject::int(i64::try_from(args.data.len()).unwrap())
                    }
                    OsFunctionCall::ReadBytes(_) => MontyObject::bytes(content.clone()),
                    other => panic!("unexpected call: {other:?}"),
                };
                call.resume(result, PrintWriter::Stdout).unwrap()
            }
            other => panic!("unexpected progress: {other:?}"),
        };
    }
}

#[test]
fn binary_update_switches_between_reads_and_writes_across_snapshots() {
    let code = r"
with open('/file', 'w+b') as f:
    assert f.read() == b''
    assert f.write(b'abcdef') == 6
    assert f.read() == b''
    f.seek(0)
    old = f.read()
    f.seek(2)
    f.write(b'XY')
    assert f.tell() == 4
    assert f.read() == b'ef'
    f.seek(0)
    assert f.read() == b'abXYef'
    assert old == b'abcdef'
    f.seek(8)
    f.write(b'Z')
    f.seek(100)
    assert f.write(b'') == 0
    assert f.seek(0, 2) == 9
    f.seek(0)
    assert f.read() == b'abXYef\x00\x00Z'
";
    run(code, false);
    run(code, true);
}

#[test]
fn write_only_binary_mode_overwrites_at_the_cursor() {
    run(
        r"
with open('/file', 'wb') as f:
    f.write(b'abcdef')
    f.seek(2)
    f.write(b'XY')
    assert f.tell() == 4
    assert f.seek(0, 2) == 6
",
        true,
    );
}
