use std::borrow::Cow;

use crate::exception_public::MontyException;

/// Identifies the output stream for a single print fragment.
///
/// Today the `print()` builtin only writes to `Stdout`. The `Stderr` variant is
/// included for forward compatibility with a future `print(..., file=sys.stderr)`
/// implementation so the collected-output API shape does not have to change when
/// that lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintStream {
    /// Standard output — the default for every `print()` call today.
    Stdout,
    /// Standard error — reserved for future `print(..., file=sys.stderr)` support.
    Stderr,
}

/// Output handler for the `print()` builtin function.
///
/// Provides common output modes as enum variants to avoid trait object overhead
/// in the typical cases (stdout, disabled, collect). For custom output handling,
/// use the `Callback` variant with a [`PrintWriterCallback`] implementation.
///
/// # Variants
/// - `Disabled` — silently discards all output (useful for benchmarking or suppressing output).
/// - `Stdout` — writes to standard output (the default behavior).
/// - `CollectString` — accumulates output into a target `String` for programmatic access.
///   No stream labels are preserved; every fragment is appended in the order it was emitted.
/// - `CollectStreams` — accumulates output as `(stream, text)` pairs, merging consecutive
///   same-stream fragments into one tuple. Each write to the same stream extends the
///   trailing entry rather than producing a new one; a new tuple is only pushed when
///   the stream changes.
/// - `Callback` — delegates to a user-provided [`PrintWriterCallback`] implementation.
pub enum PrintWriter<'a> {
    /// Silently discard all output.
    Disabled,
    /// Write to standard output.
    Stdout,
    /// Collect all output into a single `String`, in emit order, with no stream labels.
    CollectString(&'a mut String),
    /// Collect all output as `(stream, text)` tuples.
    ///
    /// The builtin `print()` implementation calls `stdout_write` for each argument
    /// and `stdout_push` for each separator/terminator. To avoid one tuple per
    /// fragment, this variant appends to the trailing tuple when it already matches
    /// the current stream; a new tuple is only pushed when the stream changes.
    /// So long as every write targets the same stream (the status quo today, since
    /// `print()` only writes to stdout), a single `print(a, b)` call produces one
    /// `(Stdout, "a b\n")` entry — and consecutive prints with `end=''` likewise
    /// merge into a single trailing entry.
    CollectStreams(&'a mut Vec<(PrintStream, String)>),
    /// Delegate to a custom callback.
    Callback(&'a mut dyn PrintWriterCallback),
}

impl PrintWriter<'_> {
    /// Creates a new `PrintWriter` that reborrows the same underlying target.
    ///
    /// This is useful in iterative execution (`start`/`resume` loops) where each
    /// step takes `PrintWriter` by value but you want all steps to write to the
    /// same output target. The original writer remains valid after the reborrowed
    /// copy is dropped.
    pub fn reborrow(&mut self) -> PrintWriter<'_> {
        match self {
            Self::Disabled => PrintWriter::Disabled,
            Self::Stdout => PrintWriter::Stdout,
            Self::CollectString(buf) => PrintWriter::CollectString(buf),
            Self::CollectStreams(buf) => PrintWriter::CollectStreams(buf),
            Self::Callback(cb) => PrintWriter::Callback(&mut **cb),
        }
    }

    /// Called once for each formatted argument passed to `print()`.
    ///
    /// This method writes only the given argument's text, without adding
    /// separators or a trailing newline. Separators (spaces) and the final
    /// terminator (newline) are emitted via [`stdout_push`](Self::stdout_push).
    pub fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        match self {
            Self::Disabled => Ok(()),
            Self::Stdout => {
                print!("{output}");
                Ok(())
            }
            Self::CollectString(buf) => {
                buf.push_str(&output);
                Ok(())
            }
            Self::CollectStreams(buf) => {
                append_streams_str(buf, PrintStream::Stdout, &output);
                Ok(())
            }
            Self::Callback(cb) => cb.stdout_write(output),
        }
    }

    /// Appends a single character to the output.
    ///
    /// Generally called to add spaces (separators) and newlines (terminators)
    /// within print output.
    pub fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        match self {
            Self::Disabled => Ok(()),
            Self::Stdout => {
                print!("{end}");
                Ok(())
            }
            Self::CollectString(buf) => {
                buf.push(end);
                Ok(())
            }
            Self::CollectStreams(buf) => {
                append_streams_char(buf, PrintStream::Stdout, end);
                Ok(())
            }
            Self::Callback(cb) => cb.stdout_push(end),
        }
    }
}

/// Appends a string fragment to the collect-streams buffer, merging into the
/// trailing tuple when the stream matches.
fn append_streams_str(buf: &mut Vec<(PrintStream, String)>, stream: PrintStream, text: &str) {
    match buf.last_mut() {
        Some((s, existing)) if *s == stream => existing.push_str(text),
        _ => buf.push((stream, text.to_owned())),
    }
}

/// Appends a single character to the collect-streams buffer, merging into the
/// trailing tuple when the stream matches.
fn append_streams_char(buf: &mut Vec<(PrintStream, String)>, stream: PrintStream, ch: char) {
    match buf.last_mut() {
        Some((s, existing)) if *s == stream => existing.push(ch),
        _ => buf.push((stream, String::from(ch))),
    }
}

/// Trait for custom output handling from the `print()` builtin function.
///
/// Implement this trait and pass it via [`PrintWriter::Callback`] to capture
/// or redirect print output from sandboxed Python code.
pub trait PrintWriterCallback {
    /// Called once for each formatted argument passed to `print()`.
    ///
    /// This method is responsible for writing only the given argument's text, and must
    /// not add separators or a trailing newline. Separators (such as spaces) and the
    /// final terminator (such as a newline) are emitted via [`stdout_push`](Self::stdout_push).
    ///
    /// # Arguments
    /// * `output` - The formatted output string for a single argument (without
    ///   separators or trailing newline).
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException>;

    /// Add a single character to stdout.
    ///
    /// Generally called to add spaces and newlines within print output.
    ///
    /// # Arguments
    /// * `end` - The character to print after the formatted output.
    fn stdout_push(&mut self, end: char) -> Result<(), MontyException>;
}
