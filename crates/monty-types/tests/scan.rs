//! Tests for the glob matching shared by the sandbox and filesystem hosts.
//!
//! Expectations for single segments come from CPython's `fnmatch` (case
//! sensitive) and `re.IGNORECASE` over `fnmatch.translate` (insensitive).

use std::{convert::Infallible, iter::repeat_n};

use monty_types::{
    MontyObject, ScanArgs,
    scan::{
        EntryInfo, GlobSelector, Listing, ScanEntry, ScanReply, ScanSource, ScanTree, SegmentPattern, collect_scan,
        parse_scan_reply, split_literal_prefix,
    },
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
        // `re.IGNORECASE` lowercases with Unicode's simple mapping (`İ` → `i`),
        // folds ranges both ways and pairs `ſ` with `s`
        ("[A-Z]", "İ", true),
        ("i", "İ", true),
        ("I", "ı", true),
        ("[A-Z]", "K", true),
        ("k", "K", true),
        ("[a-z]", "ſ", true),
        ("s", "ſ", true),
        ("[A-Z]", "ß", false),
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

/// An unterminated `[` makes every later `[` literal without rescanning for a `]`.
#[test]
fn unterminated_brackets_compile_linearly() {
    let pattern = "[".repeat(1_000_000);
    let compiled = SegmentPattern::new(&pattern, true);
    assert!(compiled.matches(&pattern));
    assert!(!compiled.matches("[]"));
    // every `[` after the first unterminated one is literal too
    let compiled = SegmentPattern::new("[a[b", true);
    assert!(compiled.matches("[a[b"));
    assert!(!compiled.matches("ab"));
}

/// Compiling is linear in the parts: only the steps the first part leads to
/// are built, each consuming the parts it covers, so a long literal tail is
/// joined once rather than once per part before it.
#[test]
fn long_literal_tails_compile_once() {
    let mut parts = vec!["**".to_owned()];
    parts.extend(repeat_n("a".to_owned(), 100_000));
    let mut tree = tree(&[("", 'd'), ("a", 'd'), ("a/a", 'f')]);
    for recurse_symlinks in [false, true] {
        let matches = all_matches(&GlobSelector::new(&parts, None, recurse_symlinks), &mut tree);
        assert_eq!(matches, Vec::<String>::new());
    }
    parts.truncate(3);
    let matches = all_matches(&GlobSelector::new(&parts, None, false), &mut tree);
    assert_eq!(matches, ["a/a"]);
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

/// Selects with `selector` from a source that never caps its matches.
fn all_matches(selector: &GlobSelector<'_>, source: &mut impl ScanSource<Error = Infallible>) -> Vec<String> {
    let Ok(matches) = selector.select(source);
    matches.expect("an uncapped source keeps every match")
}

/// Globs `pattern` over `tree`, sorted.
fn select(tree: &mut ScanTree, pattern: &str, recurse_symlinks: bool) -> Vec<String> {
    let parts: Vec<String> = pattern.split('/').map(str::to_owned).collect();
    let mut matches = all_matches(&GlobSelector::new(&parts, None, recurse_symlinks), tree);
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
    assert_eq!(select(&mut tree, "pkg/../a.py", false), ["a.py"]);
    assert_eq!(select(&mut tree, "*/../a.py", false), ["a.py"]);
    assert_eq!(select(&mut tree, "*/..", false), [""]);
    assert_eq!(select(&mut tree, "*/../..", false), Vec::<String>::new());
    assert_eq!(select(&mut tree, "a.py/", false), Vec::<String>::new());
}

/// A source that counts its listings, to show repeated pattern states read once.
struct Counting(ScanTree, usize);

impl ScanSource for Counting {
    type Error = Infallible;

    fn list(&mut self, dir: &str) -> Result<Option<Listing>, Infallible> {
        self.1 += 1;
        self.0.list(dir)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, Infallible> {
        self.0.lookup(path)
    }

    fn visit(&mut self) -> Result<(), Infallible> {
        Ok(())
    }

    fn charge(&mut self, _bytes: usize) -> Result<(), Infallible> {
        Ok(())
    }
}

#[test]
fn repeated_pattern_states_are_visited_once() {
    let mut source = Counting(tree(&[("", 'd'), ("a", 'd'), ("a/b", 'd'), ("a/b/c", 'f')]), 0);
    let parts: Vec<String> = "*/**/*/**".split('/').map(str::to_owned).collect();
    let matches = all_matches(&GlobSelector::new(&parts, None, false), &mut source);
    assert_eq!(matches, ["a/b/c", "a/b/"]);
    assert!(source.1 < 20, "listed {} times", source.1);
}

/// States are normalized, so routes through `..` collapse into one directory
/// whether a host or the sandbox selects.
#[test]
fn dot_dot_routes_collapse() {
    let names: Vec<String> = (0..3).map(|i| format!("{i:032}")).collect();
    let mut entries = vec![(String::new(), 'd')];
    entries.extend(names.iter().map(|name| (name.clone(), 'd')));
    let entries: Vec<(&str, char)> = entries.iter().map(|(path, kind)| (path.as_str(), *kind)).collect();
    let mut source = Counting(tree(&entries), 0);
    let pattern = "*/../".repeat(13) + "*";
    let parts: Vec<String> = pattern.split('/').map(str::to_owned).collect();
    all_matches(&GlobSelector::new(&parts, None, false), &mut source);
    assert!(source.1 < 100, "listed {} times", source.1);
    // CPython would spell out every route and yield 3^14 matches
    source.1 = 0;
    let mut matches = all_matches(&GlobSelector::new(&parts, None, false), &mut source);
    matches.sort();
    assert_eq!(matches, names);
    assert!(source.1 < 100, "listed {} times", source.1);
}

/// A source that totals what a scan charges it and fails past `cap` bytes.
struct Charging {
    tree: ScanTree,
    charged: usize,
    cap: usize,
}

impl ScanSource for Charging {
    type Error = usize;

    fn list(&mut self, dir: &str) -> Result<Option<Listing>, usize> {
        let Ok(listing) = self.tree.list(dir);
        Ok(listing)
    }

    fn lookup(&mut self, path: &str) -> Result<Option<EntryInfo>, usize> {
        let Ok(info) = self.tree.lookup(path);
        Ok(info)
    }

    fn visit(&mut self) -> Result<(), usize> {
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), usize> {
        self.charged += bytes;
        if self.charged > self.cap {
            Err(self.charged)
        } else {
            Ok(())
        }
    }
}

/// Glob states are charged to the source as they queue, so a pattern holding
/// one per entry for every wildcard part pays for each, and a cap stops it.
#[test]
fn glob_states_are_charged_to_the_source() {
    let names: Vec<String> = (0..50).map(|i| format!("{i:0200}")).collect();
    let mut entries = vec![(String::new(), 'd')];
    entries.extend(names.iter().map(|name| (name.clone(), 'd')));
    let entries: Vec<(&str, char)> = entries.iter().map(|(path, kind)| (path.as_str(), *kind)).collect();
    let charged = |pattern: &str, cap: usize| {
        let mut source = Charging {
            tree: tree(&entries),
            charged: 0,
            cap,
        };
        let parts = pattern.split('/').map(str::to_owned).collect();
        let args = ScanArgs::glob("".into(), parts, None, false);
        collect_scan(&args, &mut source).map(|_| source.charged)
    };
    let single = charged("*", usize::MAX).unwrap();
    let crafted = charged(&("*/../".repeat(31) + "*"), usize::MAX).unwrap();
    // each of the 31 wildcards with more parts to come queues a state per entry
    assert!(crafted > 31 * 50 * 200, "charged {crafted}");
    assert!(single < 50 * 2 * 200, "charged {single}");
    assert!(charged(&("*/../".repeat(31) + "*"), crafted / 2).is_err());
}

/// Following symlinks, a merged trailing `**` needs a segment, as CPython's `.*` after `/` does.
#[test]
fn followed_recursive_tail_matches_like_cpython() {
    let mut tree = tree(&[("", 'd'), ("a", 'd'), ("a/b", 'd'), ("a/b/f", 'f')]);
    assert_eq!(select(&mut tree, "**/a/**", true), ["a/b", "a/b/f"]);
    assert_eq!(select(&mut tree, "**/a/**", false), ["a/", "a/b", "a/b/f"]);
}

/// A host replies with a glob's matches, in the order the sandbox would select
/// them, unless they outnumber the entries read; then with those entries.
#[test]
fn collect_scan_replies_with_matches_unless_they_outnumber_the_entries() {
    let mut source = tree(&[
        ("", 'd'),
        ("b", 'd'),
        ("b/x.py", 'f'),
        ("a", 'd'),
        ("a/y.py", 'f'),
        ("a/z.txt", 'f'),
        ("a/c", 'd'),
        ("a/c/w.py", 'f'),
    ]);
    let parts = |pattern: &str| pattern.split('/').map(str::to_owned).collect::<Vec<_>>();
    let selective = parts("**/*.py");
    let Ok(reply) = collect_scan(&ScanArgs::glob("".into(), selective.clone(), None, false), &mut source);
    let expected = all_matches(&GlobSelector::new(&selective, None, false), &mut source);
    assert_eq!(expected.len(), 3);
    assert_eq!(reply, Some(ScanReply::Matches(expected)));

    // `**/*/**` yields each entry once per directory above it
    let amplifying = parts("**/*/**");
    let Ok(reply) = collect_scan(&ScanArgs::glob("".into(), amplifying, None, false), &mut source);
    let Some(ScanReply::Tree(entries)) = reply else {
        panic!("expected the tree, got {reply:?}");
    };
    assert_eq!(entries.len(), 8);
}

/// Matches round-trip through their encoding, normalized but keeping a
/// directory's trailing `/`; one that leaves the scan root is refused.
#[test]
fn matches_replies_round_trip_and_stay_below_the_root() {
    let reply = ScanReply::Matches(vec!["a/".to_owned(), "a/./b".to_owned(), String::new()]);
    assert_eq!(
        parse_scan_reply(&reply.into_object()),
        Ok(ScanReply::Matches(vec![
            "a/".to_owned(),
            "a/b".to_owned(),
            String::new()
        ]))
    );
    for path in ["/etc", "../x", "a/../../x"] {
        let reply = ScanReply::Matches(vec![path.to_owned()]).into_object();
        assert_eq!(
            parse_scan_reply(&reply),
            Err(format!("glob match {path:?} is not relative to the scan root"))
        );
    }
    let other = MontyObject::dict([(MontyObject::string("entries".to_owned()), MontyObject::list([]))]);
    assert_eq!(
        parse_scan_reply(&other),
        Err("expected {'matches': [path, ...]}, got a different dict".to_owned())
    );
}
