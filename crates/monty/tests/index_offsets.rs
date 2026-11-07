use std::{borrow::Cow, thread, time::Duration};

use monty::MontyRepl;
use monty_types::{
    CompileOptions, ExcType, MontyException, MontyObject, PrintWriter, PrintWriterCallback, ResourceTracker,
};

/// Delays the first equality callback so the native search must observe an expired budget.
struct DelayedOutput {
    text: String,
    delay: Option<Duration>,
}

impl PrintWriterCallback for DelayedOutput {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        if let Some(delay) = self.delay.take() {
            thread::sleep(delay);
        }
        self.text.push_str(&output);
        Ok(())
    }

    fn stdout_push(&mut self, output: char) -> Result<(), MontyException> {
        self.text.push(output);
        Ok(())
    }
}

/// Python callbacks reset the dispatch countdown, so the offset iterator must keep its own poll.
#[test]
fn index_offsets_keep_callback_timeout_checks() {
    for constructor in ["list", "tuple"] {
        let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
        let setup = format!(
            "class Needle:\n    def __eq__(self, other):\n        print('.', end='')\n        return False\nneedle = Needle()\nvalues = {constructor}(range(4096))"
        );
        repl.feed_run(&setup, vec![], PrintWriter::Stdout).unwrap();
        repl.tracker_mut().set_max_feed_duration(Duration::from_millis(50));
        let mut output = DelayedOutput {
            text: String::new(),
            delay: Some(Duration::from_millis(100)),
        };
        let error = repl
            .feed_run("values.index(needle, 130)", vec![], PrintWriter::Callback(&mut output))
            .expect_err("the native search must poll while callbacks reset the dispatch countdown");
        assert_eq!(error.exc_type(), ExcType::TimeoutError, "{constructor}");
        assert!(
            (1..=ResourceTracker::LOOP_CHECK_INTERVAL).contains(&output.text.len()),
            "{constructor}: {} comparisons after the budget expired",
            output.text.len()
        );

        repl.tracker_mut().set_max_feed_duration(Duration::from_secs(1));
        assert_eq!(
            repl.feed_run("1 + 1", vec![], PrintWriter::Stdout).unwrap(),
            MontyObject::int(2)
        );
    }
}
