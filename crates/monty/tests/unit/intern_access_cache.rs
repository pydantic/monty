use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

use super::*;
use crate::MontyRepl;

#[test]
fn interned_endpoint_reads_do_not_allocate_cache() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "text = '中é🐍'\nclass First:\n    def __index__(self):\n        return 0\nlen(text)\ntext[0]\ntext[-1]\ntext[First()]\ntext.startswith('中')\ntext.endswith('🐍')",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    assert!(!repl.heap_and_interns().1.string_access_caches.is_initialized());

    repl.feed_run("text[1]", vec![], PrintWriter::Disabled).unwrap();
    assert!(repl.heap_and_interns().1.string_access_caches.is_initialized());
}

#[test]
fn ascii_entries_keep_the_pre_cache_footprint() {
    assert_eq!(
        size_of::<InternedString>(),
        size_of::<(Option<StaticStrings>, Option<NonZeroU32>, WithHash<InternedText>)>()
    );
    let interns = Interns::new("");
    for i in 0..interns.strings.len() {
        assert!(
            interns
                .str_access_cache(next_string_id(i), &ResourceTracker::default())
                .is_none()
        );
    }
    assert!(!interns.string_access_caches.is_initialized());
}
