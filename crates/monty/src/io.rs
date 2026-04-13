use std::borrow::Cow;

use crate::exception_public::MontyException;

/// Output handler for the `print()` builtin function.
///
/// Provides common output modes as enum variants to avoid trait object overhead
/// in the typical cases (stdout, disabled, collect). For custom output handling,
/// use the `Callback` variant with a [`PrintWriterCallback`] implementation.
///
/// # Variants
/// - `Disabled` - Silently discards all output (useful for benchmarking or suppressing output)
/// - `Stdout` - Writes to standard output (the default behavior)
/// - `Collect` - Accumulates output into a target `String` for programmatic access
/// - `Callback` - Delegates to a user-provided [`PrintWriterCallback`] implementation
pub enum PrintWriter<'a> {
    /// Silently discard all output.
    Disabled,
    /// Write to standard output.
    Stdout,
    /// Collect all output into a string.
    Collect(&'a mut String),
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
            Self::Collect(buf) => PrintWriter::Collect(buf),
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
            Self::Collect(buf) => {
                buf.push_str(&output);
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
            Self::Collect(buf) => {
                buf.push(end);
                Ok(())
            }
            Self::Callback(cb) => cb.stdout_push(end),
        }
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
