//! Print-output plumbing: [`PrintStream`], [`PrintWriter`] and the
//! [`PrintWriterCallback`] trait used by hosts to capture `print()` output.

use std::borrow::Cow;

use crate::{
    exceptions::{ExcType, MontyException},
    resource::ResourceError,
};

/// Default cap for [`PrintWriter::CollectString`] / [`PrintWriter::CollectStreams`]
/// and the matching Python collectors.
///
/// Host-side print buffers sit outside [`crate::ResourceLimits::max_memory`];
/// without a cap, a print loop can OOM the host while sandbox limits stay green.
/// Pass `max_bytes: None` to opt out on trusted hosts.
pub const DEFAULT_MAX_PRINT_COLLECT_BYTES: usize = 10 * 1024 * 1024;

/// Identifies the output stream for a single print fragment.
///
/// `print()` writes to `Stdout` unless it is given `file=sys.stderr`, which is
/// the only way sandboxed code can reach `Stderr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintStream {
    /// Standard output, the default for a `print()` call with no `file=`.
    Stdout,
    /// Standard error, reached by `print(..., file=sys.stderr)`.
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
///   The `Option<usize>` is an optional byte cap (`None` = unlimited); constructors
///   [`collect_string`](Self::collect_string) / default Python collectors use
///   [`DEFAULT_MAX_PRINT_COLLECT_BYTES`].
/// - `CollectStreams` — accumulates output as `(stream, text)` pairs, merging consecutive
///   same-stream fragments into one tuple. Each write to the same stream extends the
///   trailing entry rather than producing a new one; a new tuple is only pushed when
///   the stream changes. Same optional byte cap as `CollectString`.
/// - `Callback` — delegates to a user-provided [`PrintWriterCallback`] implementation.
pub enum PrintWriter<'a> {
    /// Silently discard all output.
    Disabled,
    /// Write to standard output.
    Stdout,
    /// Collect all output into a single `String`, in emit order, with no stream labels.
    ///
    /// Second field: max collected bytes (`None` = unlimited). Exceeding raises
    /// `MemoryError` with the same message as [`ResourceError::Memory`].
    CollectString(&'a mut String, Option<usize>),
    /// Collect all output as `(stream, text)` entries.
    ///
    /// The builtin `print()` implementation calls `write` for each argument and
    /// `push` for each separator/terminator. To avoid one entry per fragment,
    /// [`CollectedStreams`] appends to the trailing entry when it already
    /// matches the current stream; a new entry is only pushed when the stream
    /// changes. So a run that stays on one stream collects a single entry
    /// however many fragments it wrote, and one that alternates collects one
    /// per run.
    ///
    /// Second field: max collected bytes across all entries (`None` = unlimited).
    CollectStreams(&'a mut CollectedStreams, Option<usize>),
    /// Delegate to a custom callback.
    Callback(&'a mut dyn PrintWriterCallback),
}

impl PrintWriter<'_> {
    /// Collect into `buf` with the default [`DEFAULT_MAX_PRINT_COLLECT_BYTES`] cap.
    pub fn collect_string(buf: &mut String) -> PrintWriter<'_> {
        PrintWriter::CollectString(buf, Some(DEFAULT_MAX_PRINT_COLLECT_BYTES))
    }

    /// Collect into `buf` with the default [`DEFAULT_MAX_PRINT_COLLECT_BYTES`] cap.
    pub fn collect_streams(buf: &mut CollectedStreams) -> PrintWriter<'_> {
        PrintWriter::CollectStreams(buf, Some(DEFAULT_MAX_PRINT_COLLECT_BYTES))
    }

    /// Creates a new [`PrintWriter`] that reborrows the same underlying target.
    ///
    /// This is useful in iterative execution (`start`/`resume` loops) where each
    /// step takes [`PrintWriter`] by value but you want all steps to write to the
    /// same output target. The original writer remains valid after the reborrowed
    /// copy is dropped.
    pub fn reborrow(&mut self) -> PrintWriter<'_> {
        match self {
            Self::Disabled => PrintWriter::Disabled,
            Self::Stdout => PrintWriter::Stdout,
            Self::CollectString(buf, max) => PrintWriter::CollectString(buf, *max),
            Self::CollectStreams(buf, max) => PrintWriter::CollectStreams(buf, *max),
            Self::Callback(cb) => PrintWriter::Callback(&mut **cb),
        }
    }

    /// Called once for each formatted argument passed to `print()`.
    ///
    /// This method writes only the given argument's text, without adding
    /// separators or a trailing newline. Separators (spaces) and the final
    /// terminator (newline) are emitted via [`push`](Self::push).
    ///
    /// `CollectString` keeps no stream labels, so a run that prints to both
    /// streams interleaves them in one buffer. Use `CollectStreams` to tell
    /// them apart.
    pub fn write(&mut self, stream: PrintStream, output: Cow<'_, str>) -> Result<(), MontyException> {
        match self {
            Self::Disabled => Ok(()),
            Self::Stdout => {
                match stream {
                    PrintStream::Stdout => print!("{output}"),
                    PrintStream::Stderr => eprint!("{output}"),
                }
                Ok(())
            }
            Self::CollectString(buf, max_bytes) => {
                check_print_collect_limit(buf.len(), output.len(), *max_bytes)?;
                buf.push_str(&output);
                Ok(())
            }
            Self::CollectStreams(buf, max_bytes) => buf.push_str(stream, &output, *max_bytes),
            Self::Callback(cb) => match stream {
                PrintStream::Stdout => cb.stdout_write(output),
                PrintStream::Stderr => cb.stderr_write(output),
            },
        }
    }

    /// Appends a single character to the given stream.
    ///
    /// Generally called to add spaces (separators) and newlines (terminators)
    /// within print output.
    pub fn push(&mut self, stream: PrintStream, end: char) -> Result<(), MontyException> {
        match self {
            Self::Disabled => Ok(()),
            Self::Stdout => {
                match stream {
                    PrintStream::Stdout => print!("{end}"),
                    PrintStream::Stderr => eprint!("{end}"),
                }
                Ok(())
            }
            Self::CollectString(buf, max_bytes) => {
                check_print_collect_limit(buf.len(), end.len_utf8(), *max_bytes)?;
                buf.push(end);
                Ok(())
            }
            Self::CollectStreams(buf, max_bytes) => buf.push_char(stream, end, *max_bytes),
            Self::Callback(cb) => match stream {
                PrintStream::Stdout => cb.stdout_push(end),
                PrintStream::Stderr => cb.stderr_push(end),
            },
        }
    }

    /// [`write`](Self::write) to [`PrintStream::Stdout`].
    pub fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.write(PrintStream::Stdout, output)
    }

    /// [`push`](Self::push) to [`PrintStream::Stdout`].
    pub fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        self.push(PrintStream::Stdout, end)
    }

    /// Whether this writer wants [`poll_flush`](Self::poll_flush) called at all.
    ///
    /// Only `Callback` can buffer, so the VM hoists this out of its dispatch
    /// loop and skips the poll (and its clock read) entirely for every other
    /// variant.
    #[must_use]
    pub fn wants_poll(&self) -> bool {
        matches!(self, Self::Callback(_))
    }

    /// Forwards the VM's periodic checkpoint to a buffering callback.
    pub fn poll_flush(&mut self) -> Result<(), MontyException> {
        match self {
            Self::Callback(cb) => cb.poll_flush(),
            _ => Ok(()),
        }
    }
}

/// The buffer behind [`PrintWriter::CollectStreams`]: `(stream, text)` entries
/// plus the byte total their cap is checked against.
///
/// The total is carried rather than re-derived because the cap is checked on
/// every fragment: summing the entries each time is O(entries), which a run
/// alternating between the streams turns into quadratic work, since each switch
/// starts a new entry. `pydantic_monty`'s collector keeps the same running
/// charge for the same reason.
#[derive(Debug, Default)]
pub struct CollectedStreams {
    entries: Vec<(PrintStream, String)>,
    /// UTF-8 bytes across `entries`, kept in step with every append.
    bytes: usize,
}

impl CollectedStreams {
    /// The collected fragments, in the order the sandbox produced them.
    #[must_use]
    pub fn entries(&self) -> &[(PrintStream, String)] {
        &self.entries
    }

    /// Takes the collected fragments, leaving the buffer empty.
    #[must_use]
    pub fn into_entries(self) -> Vec<(PrintStream, String)> {
        self.entries
    }

    /// Appends a string fragment, merging into the trailing entry when the
    /// stream matches.
    fn push_str(&mut self, stream: PrintStream, text: &str, max_bytes: Option<usize>) -> Result<(), MontyException> {
        self.charge(text.len(), max_bytes)?;
        match self.entries.last_mut() {
            Some((s, existing)) if *s == stream => existing.push_str(text),
            _ => self.entries.push((stream, text.to_owned())),
        }
        Ok(())
    }

    /// Appends a single character, merging into the trailing entry when the
    /// stream matches.
    fn push_char(&mut self, stream: PrintStream, ch: char, max_bytes: Option<usize>) -> Result<(), MontyException> {
        self.charge(ch.len_utf8(), max_bytes)?;
        match self.entries.last_mut() {
            Some((s, existing)) if *s == stream => existing.push(ch),
            _ => self.entries.push((stream, String::from(ch))),
        }
        Ok(())
    }

    /// Checks `add` bytes against the cap and books them, leaving the total
    /// untouched when the cap refuses them.
    fn charge(&mut self, add: usize, max_bytes: Option<usize>) -> Result<(), MontyException> {
        check_print_collect_limit(self.bytes, add, max_bytes)?;
        self.bytes = self.bytes.saturating_add(add);
        Ok(())
    }
}

/// Rejects a collect-buffer growth that would exceed `max_bytes`.
///
/// `None` means unlimited. On overflow, returns the same `MemoryError` message
/// as [`ResourceError::Memory`] so hosts see one familiar limit string.
pub fn check_print_collect_limit(
    current_len: usize,
    add: usize,
    max_bytes: Option<usize>,
) -> Result<(), MontyException> {
    let Some(limit) = max_bytes else {
        return Ok(());
    };
    let used = current_len.saturating_add(add);
    if used > limit {
        Err(MontyException::new(
            ExcType::MemoryError,
            Some(ResourceError::Memory { limit, used }.to_string()),
        ))
    } else {
        Ok(())
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

    /// Called for each formatted argument of a `print(..., file=sys.stderr)`.
    ///
    /// Defaults to [`stdout_write`](Self::stdout_write), so a host written
    /// before stderr existed still receives the text instead of losing it.
    fn stderr_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        self.stdout_write(output)
    }

    /// Adds a single character to stderr. Defaults to
    /// [`stdout_push`](Self::stdout_push), as [`stderr_write`](Self::stderr_write) does.
    fn stderr_push(&mut self, end: char) -> Result<(), MontyException> {
        self.stdout_push(end)
    }

    /// Gives a buffering implementation a chance to release what it holds.
    ///
    /// The VM calls this from its periodic dispatch checkpoint, so a callback
    /// that batches writes can bound how long output sits unsent while the
    /// program computes without printing. Implementations that write straight
    /// through do nothing here.
    fn poll_flush(&mut self) -> Result<(), MontyException> {
        Ok(())
    }
}
