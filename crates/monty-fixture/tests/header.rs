use insta::assert_snapshot;
use monty_fixture::{Bench, Header, Interpreter};

/// The error message from parsing `source`, which must fail.
fn parse_error(source: &str) -> String {
    Header::parse(source).unwrap_err().to_string()
}

#[test]
fn no_header_is_default() {
    assert_eq!(Header::parse("# a comment\nx = 1\n").unwrap(), Header::default());
}

#[test]
fn reads_every_key() {
    let source = "\
# leading prose
# /// monty
# call-external = true
# xfail = ['monty', 'cpython']
# gc-interval = 200
# timezone = 'Europe/London'  # trailing comment
#
# [bench]
# data = '../input.json'
# limits = true
# ///
x = 1
";
    let header = Header::parse(source).unwrap();
    assert_eq!(
        header,
        Header {
            call_external: true,
            xfail: vec![Interpreter::Monty, Interpreter::Cpython],
            gc_interval: Some(200),
            timezone: Some("Europe/London".to_owned()),
            bench: Bench {
                data: Some("../input.json".to_owned()),
                limits: true,
                ..Bench::default()
            },
            ..Header::default()
        }
    );
    assert!(header.xfails(Interpreter::Cpython));
}

#[test]
fn rejects_unknown_key() {
    assert_snapshot!(parse_error("# /// monty\n# xfial = ['monty']\n# ///\n"), @"
    in the header opening on line 1: TOML parse error at line 1, column 1
      |
    1 | xfial = ['monty']
      | ^^^^^
    unknown field `xfial`, expected one of `call-external`, `run-async`, `mount-fs`, `skip-cpython-windows`, `cpython-main-module`, `xfail`, `gc-interval`, `timezone`, `bench`
    ");
}

#[test]
fn rejects_unknown_bench_key() {
    assert_snapshot!(parse_error("# /// monty\n# [bench]\n# mount-fs = true\n# ///\n"), @"
    in the header opening on line 1: TOML parse error at line 2, column 1
      |
    2 | mount-fs = true
      | ^^^^^^^^
    unknown field `mount-fs`, expected one of `data`, `mount`, `overlay`, `limits`
    ");
}

#[test]
fn rejects_block_after_code() {
    assert_snapshot!(parse_error("x = 1\n# /// monty\n# run-async = true\n# ///\n"), @"line 2: `# /// monty` must come before any code");
}

#[test]
fn rejects_block_inside_a_string() {
    assert_snapshot!(parse_error("s = '''\n# /// monty\n# run-async = true\n# ///\n'''\n"), @"line 2: `# /// monty` must come before any code");
}

#[test]
fn rejects_misspelt_marker() {
    assert_snapshot!(parse_error("#/// monty\n# run-async = true\n# ///\n"), @"line 1: `#/// monty` is not `# /// monty` or a block's closing `# ///`");
}

#[test]
fn rejects_second_block() {
    assert_snapshot!(parse_error("# /// monty\n# ///\n# /// monty\n# ///\n"), @"line 3: second `# /// monty` block");
}

#[test]
fn rejects_unclosed_block() {
    assert_snapshot!(parse_error("# /// monty\n# run-async = true\n"), @"line 1: `# /// monty` is never closed");
}

#[test]
fn rejects_non_comment_body_line() {
    assert_snapshot!(parse_error("# /// monty\n#run-async = true\n# ///\n"), @"line 2: `#run-async = true` inside the header is not a `# ` comment");
}
