use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

use super::*;
use crate::MontyRepl;

#[test]
fn short_interned_strings_and_long_endpoint_reads_skip_the_cache() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let short = "é".repeat(32);
    let code = format!(
        "short = '{short}'\ntext = '{short}🐍'\nclass First:\n    def __index__(self):\n        return 0\n\
         assert short[1] == 'é'\nassert short[8:10] == 'éé'\nassert short.rfind('é', 8) == 31\n\
         len(text)\ntext[0]\ntext[-1]\ntext[First()]\ntext.startswith('é')\ntext.endswith('🐍')"
    );
    repl.feed_run(&code, vec![], PrintWriter::Disabled).unwrap();
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
