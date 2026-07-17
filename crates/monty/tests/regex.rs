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
use monty::{CompileOptions, MontyRun};

/// Helper to run Python code and return the string result.
fn run(code: &str) -> String {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
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
