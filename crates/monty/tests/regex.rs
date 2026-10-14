/// Tests for regex behavior that cannot be tested via the datatest runner
/// (which runs tests against both CPython and Monty) — engine divergences from
/// CPython, and cases too slow for the runner's per-file timeout.
/// In particular, `fancy_regex` enforces a backtrack limit that CPython lacks,
/// so pathological patterns raise `PatternError` in Monty instead of hanging.
///
/// CPython's regex engine uses backtracking with no step limit. Pathological
/// patterns (e.g. `((a+)\2)+b` against 50+ 'a's) cause exponential-time hangs
/// that grow unboundedly — a denial-of-service vector. Monty uses `fancy_regex`
/// which enforces a default 1M-step backtrack limit, raising `re.PatternError`
/// when exceeded. This is strictly better behavior for a sandbox.
use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

/// Helper to run Python code and return the string result.
fn run(code: &str) -> String {
    let mut ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let result = ex.run_no_limits(vec![]).unwrap();
    let s: String = result.as_ref().try_into().unwrap();
    s
}

/// Verify that `fancy_regex`'s backtrack limit prevents ReDoS.
///
/// CPython's regex engine has no backtrack limit, so pathological patterns with
/// backreferences cause exponential-time hangs (e.g. `((a+)\2)+b` against 40 'a's
/// takes ~0.17s on CPython and doubles with each additional character, making it
/// completely unusable at ~50+ characters and a denial-of-service vector).
///
/// Monty uses `fancy_regex` which enforces a default 1M-step backtrack limit.
/// Patterns that exceed this limit raise `re.PatternError` instead of hanging,
/// making the sandbox safe against ReDoS attacks via backreference-based patterns.
///
/// Note: `fancy_regex` delegates simple patterns (no backreferences or lookaround)
/// to the `regex` crate's DFA engine, which guarantees linear-time matching.
/// The backtrack limit only applies to patterns that require the backtracking engine.
#[test]
fn backtrack_limit_prevents_redos() {
    // Pattern with backreference forces the backtracking engine.
    // ((a+)\2)+b tries to match repeated groups of a's where each group
    // is followed by its own backreference, then a 'b' that never appears.
    // This creates exponential backtracking paths.
    let result = run(r"
import re
try:
    re.search(r'((a+)\2)+b', 'a' * 40 + 'c')
    result = 'no error'
except re.PatternError as e:
    result = str(e)
result
");
    assert_eq!(
        result,
        "Error executing regex: Max limit for backtracking count exceeded"
    );
}

/// Verify that the backtrack limit also applies to compiled patterns.
#[test]
fn backtrack_limit_on_compiled_pattern() {
    let result = run(r"
import re
p = re.compile(r'((a+)\2)+b')
try:
    p.search('a' * 40 + 'c')
    result = 'no error'
except re.PatternError as e:
    result = str(e)
result
");
    assert_eq!(
        result,
        "Error executing regex: Max limit for backtracking count exceeded"
    );
}

/// Verify that non-fancy patterns (no backreferences/lookaround) are delegated
/// to the DFA engine and don't hit the backtrack limit even with large inputs.
#[test]
fn dfa_engine_handles_large_inputs() {
    // (a+)+b is pathological for backtracking engines but fancy_regex delegates
    // it to the regex crate's DFA engine since it has no fancy features.
    let result = run(r"
import re
m = re.search(r'(a+)+b', 'a' * 10000 + 'c')
assert m is None, 'no match expected'
'ok'
");
    assert_eq!(result, "ok");
}

/// Patterns whose compiled form exceeds the pattern cache's per-entry
/// `delegate_size_limit` are recompiled per call instead of retained — they must
/// still match identically. Lives here rather than `test_cases/` because
/// debug-build compilation of the expanded counted repeats is too slow for the
/// datatest runner's per-file timeout.
#[test]
fn oversize_pattern_not_cached_still_matches() {
    let result = run(r"
import re
m = re.fullmatch('(?:ab){3000}', 'ab' * 3000)
assert m is not None, 'oversize counted repeat fullmatch succeeds'
assert m.span() == (0, 6000), 'oversize counted repeat span'
assert re.findall('a{5000}', 'a' * 5000) == ['a' * 5000], 'oversize pattern findall'
'ok'
");
    assert_eq!(result, "ok");
}

/// `finditer` searches lazily, so a match before a pathological stretch is
/// returned and the backtrack limit only fires on the `next()` that reaches it.
#[test]
fn finditer_backtrack_limit_raises_on_next() {
    let result = run(r"
import re
it = re.finditer(r'((a+)\2)+b|c', 'c' + 'a' * 40 + 'c')
assert next(it).span() == (0, 1)
try:
    next(it)
    result = 'no error'
except re.PatternError as e:
    result = str(e)
result
");
    assert_eq!(
        result,
        "Error executing regex: Max limit for backtracking count exceeded"
    );
}

/// An invalid pattern is reported before a bad subject, as in CPython; the
/// message is `fancy_regex`'s, so this can't run against CPython.
#[test]
fn finditer_pattern_error_precedes_subject_error() {
    let result = run(r"
import re
try:
    re.finditer('(', 1)
    result = 'no error'
except re.PatternError as e:
    result = str(e)
result
");
    assert_eq!(
        result,
        "Parsing error at position 1: Opening parenthesis without closing parenthesis"
    );
}

/// The empty-match rule documented in `limitations/re.md`: an empty match where
/// the previous match ended is skipped, so CPython's `(2, 2)` is missing.
#[test]
fn finditer_skips_empty_match_at_previous_end() {
    let result = run(r"
import re
str([m.span() for m in re.finditer(r'x*', 'axb')])
");
    assert_eq!(result, "[(0, 0), (1, 2), (3, 3)]");
}

/// A partly consumed `finditer` survives a session dump and resumes where it stopped.
#[test]
fn finditer_resumes_after_dump_and_load() {
    let mut repl = MontyRepl::new("repl.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "import re\nit = re.finditer(r'\\d+', 'a1 b22 c333')\nnext(it)",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("repl.py", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut repl) = Dump::load(&bytes).unwrap().state else {
        panic!("dumped an idle session, loaded something else")
    };
    let rest = repl
        .feed_run("[m.group() for m in it]", vec![], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(
        rest,
        MontyObject::list(vec![MontyObject::string("22"), MontyObject::string("333")])
    );
}
