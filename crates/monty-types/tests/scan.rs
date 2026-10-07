//! Tests for the glob matching shared by the sandbox and filesystem hosts.
//!
//! Expectations for single segments come from CPython's `fnmatch` (case
//! sensitive) and `re.IGNORECASE` over `fnmatch.translate` (insensitive).

use std::convert::Infallible;

use monty_types::scan::{
    EntryInfo, GlobSelector, ScanEntry, ScanSource, ScanTree, SegmentPattern, split_literal_prefix,
};

#[test]
fn segment_patterns_match_like_fnmatch() {
    let cases = [
        ("*", "", true),
        ("*", "abc", true),
        ("*", ".hidden", true),
        ("?", "a", true),
        ("?", "", false),
        ("a*b", "ab", true),
        ("a*b", "axxb", true),
        ("a*b", "axxbc", false),
        ("*a*b*", "xxaxxbxx", true),
        ("*a*b", "ab_ab_a", false),
        ("[abc]", "b", true),
        ("[abc]", "d", false),
        ("[!abc]", "d", true),
        ("[!abc]", "a", false),
        ("[a-c]", "b", true),
        ("[a-c]", "d", false),
        ("[a-]", "-", true),
        ("[-a]", "-", true),
        ("[z-a]", "m", false),
        ("[c-az]", "z", true),
        ("[c-az]", "b", false),
        ("[]]", "]", true),
        ("[!]]", "a", true),
        ("[!]", "[!]", true),
        ("[", "[", true),
        ("[a", "[a", true),
        ("a\\b", "a\\b", true),
        ("[\\]", "\\", true),
        ("[a-c-e]", "-", true),
        ("[a-c-e]", "d", false),
        ("[a-c-e]", "e", true),
        ("[!b-a]", "x", true),
        ("[^a]", "^", true),
        ("[^a]", "b", false),
        ("a**b", "axyb", true),
        ("[*]", "*", true),
        ("[?]", "a", false),
        ("é?", "éx", true),
        ("***", "x", true),
    ];
    for (pattern, name, expected) in cases {
        assert_eq!(
            SegmentPattern::new(pattern, true).matches(name),
            expected,
            "{pattern:?} vs {name:?}"
        );
    }
}

#[test]
fn case_insensitive_segments_fold_one_character_at_a_time() {
    let cases = [
        ("A.TXT", "a.txt", true),
        ("[A-C]x", "bX", true),
        ("ß", "SS", false),
        ("Straße", "STRASSE", false),
        ("[!A]", "a", false),
        ("ǅ", "ǆ", true),
    ];
    for (pattern, name, expected) in cases {
        assert_eq!(
            SegmentPattern::new(pattern, false).matches(name),
            expected,
            "{pattern:?} vs {name:?}"
        );
    }
}

/// Matching never backtracks exponentially, whatever the sandbox sends.
#[test]
fn pathological_segment_patterns_are_linear() {
    let pattern = "*a".repeat(100) + "b";
    let name = "a".repeat(10_000);
    assert!(!SegmentPattern::new(&pattern, true).matches(&name));
}

#[test]
fn literal_prefixes_are_split_off() {
    let parts = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
    let pattern = parts(&["src", "pkg", "*.py"]);
    assert_eq!(
        split_literal_prefix(&pattern, None),
        ("src/pkg".to_owned(), &pattern[2..])
    );
    let pattern = parts(&["..", "a", "**", "b"]);
    assert_eq!(split_literal_prefix(&pattern, None), ("../a".to_owned(), &pattern[2..]));
    let pattern = parts(&["sub", ""]);
    assert_eq!(split_literal_prefix(&pattern, None), ("sub/".to_owned(), &pattern[2..]));
    // an explicit `case_sensitive` keeps literals, but not `..`, for listing
    let pattern = parts(&["..", "src", "*.py"]);
    assert_eq!(
        split_literal_prefix(&pattern, Some(true)),
        ("..".to_owned(), &pattern[1..])
    );
}

/// A tree of `(path, kind)`, kind `d`(ir), `f`(ile) or `l`(ink to a directory).
fn tree(items: &[(&str, char)]) -> ScanTree {
    ScanTree::new(
        items
            .iter()
            .map(|(path, kind)| ScanEntry {
                path: (*path).to_owned(),
                info: EntryInfo {
                    is_dir: *kind != 'f',
                    is_file: *kind == 'f',
                    is_symlink: *kind == 'l',
                },
            })
            .collect(),
    )
}

/// Globs `pattern` over `tree`, sorted.
fn select(tree: &mut ScanTree, pattern: &str, recurse_symlinks: bool) -> Vec<String> {
    let parts: Vec<String> = pattern.split('/').map(str::to_owned).collect();
    let Ok(mut matches) = GlobSelector::new(&parts, None, recurse_symlinks).select(tree);
    matches.sort();
    matches
}

#[test]
fn selector_follows_cpython_semantics() {
    let mut tree = tree(&[
        ("", 'd'),
        ("a.py", 'f'),
        (".hidden.py", 'f'),
        ("pkg", 'd'),
        ("pkg/b.py", 'f'),
        ("pkg/sub", 'd'),
        ("pkg/sub/c.py", 'f'),
        ("link", 'l'),
        ("link/b.py", 'f'),
    ]);
    assert_eq!(select(&mut tree, "*.py", false), [".hidden.py", "a.py"]);
    assert_eq!(
        select(&mut tree, "**/*.py", false),
        [".hidden.py", "a.py", "pkg/b.py", "pkg/sub/c.py"]
    );
    assert_eq!(select(&mut tree, "*/b.py", false), ["link/b.py", "pkg/b.py"]);
    assert_eq!(select(&mut tree, "**/", false), ["", "pkg/", "pkg/sub/"]);
    assert_eq!(select(&mut tree, "**/", true), ["", "link/", "pkg/", "pkg/sub/"]);
    assert_eq!(select(&mut tree, "**/b.py", true), ["link/b.py", "pkg/b.py"]);
    assert_eq!(select(&mut tree, "pkg/**/*.py", true), ["pkg/b.py", "pkg/sub/c.py"]);
    assert_eq!(select(&mut tree, "pkg/../a.py", false), ["pkg/../a.py"]);
    assert_eq!(select(&mut tree, "a.py/", false), Vec::<String>::new());
}

/// A source that counts its listings, to show repeated pattern states read once.
struct Counting(ScanTree, usize);

impl ScanSource for Counting {
    type Error = Infallible;

    fn list(&mut self, dir: &str) -> Result<Option<Vec<(String, EntryInfo)>>, Infallible> {
        self.1 += 1;
        self.0.list(dir)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, Infallible> {
        self.0.lookup(path)
    }
}

#[test]
fn repeated_pattern_states_are_visited_once() {
    let mut source = Counting(tree(&[("", 'd'), ("a", 'd'), ("a/b", 'd'), ("a/b/c", 'f')]), 0);
    let parts: Vec<String> = "*/**/*/**".split('/').map(str::to_owned).collect();
    let Ok(matches) = GlobSelector::new(&parts, None, false).select(&mut source);
    assert_eq!(matches, ["a/b/c", "a/b/"]);
    assert!(source.1 < 20, "listed {} times", source.1);
}

/// A host prunes with normalized states, so routes through `..` collapse.
#[test]
fn pruning_collapses_dot_dot_routes() {
    let names: Vec<String> = (0..3).map(|i| format!("{i:032}")).collect();
    let mut entries = vec![(String::new(), 'd')];
    entries.extend(names.iter().map(|name| (name.clone(), 'd')));
    let entries: Vec<(&str, char)> = entries.iter().map(|(path, kind)| (path.as_str(), *kind)).collect();
    let mut source = Counting(tree(&entries), 0);
    let pattern = "*/../".repeat(13) + "*";
    let parts: Vec<String> = pattern.split('/').map(str::to_owned).collect();
    GlobSelector::new(&parts, None, false).prune(&mut source).unwrap();
    assert!(source.1 < 100, "listed {} times", source.1);
    // the sandbox side still spells every route, as CPython does
    let Ok(matches) = GlobSelector::new(&parts[..parts.len() - 6], None, false).select(&mut source);
    assert_eq!(matches.len(), 3usize.pow(11));
}

/// Following symlinks, a merged trailing `**` needs a segment, as CPython's `.*` after `/` does.
#[test]
fn followed_recursive_tail_matches_like_cpython() {
    let mut tree = tree(&[("", 'd'), ("a", 'd'), ("a/b", 'd'), ("a/b/f", 'f')]);
    assert_eq!(select(&mut tree, "**/a/**", true), ["a/b", "a/b/f"]);
    assert_eq!(select(&mut tree, "**/a/**", false), ["a/", "a/b", "a/b/f"]);
}
