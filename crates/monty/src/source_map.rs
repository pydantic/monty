use std::{collections::HashMap, sync::Arc};

use memchr::memchr2_iter;
use monty_types::{CodeLoc, StackFrame};

use crate::{exception_private::RawStackFrame, intern::Interns, parse::CodeRange};

/// Byte offsets where each line starts. `\n`, `\r\n` and a bare `\r` each end
/// a line, as they do for the parser.
fn line_starts(bytes: &[u8]) -> Vec<u32> {
    let mut starts = Vec::with_capacity(bytes.len() / 40 + 1);
    starts.push(0);
    for i in memchr2_iter(b'\n', b'\r', bytes) {
        // the `\r` of a `\r\n` defers to its `\n`, so the pair ends one line
        if !(bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n')) {
            // source should never exceed 4 GB
            starts.push(u32::try_from(i + 1).unwrap_or(u32::MAX));
        }
    }
    starts
}

/// Counts the characters in `bytes`: every byte that does not continue a
/// multi-byte UTF-8 sequence, which holds even off a char boundary.
fn count_chars(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b & 0b1100_0000 != 0b1000_0000).count()
}

/// Resolver from raw byte offsets (stored on every [`CodeRange`]) back to
/// line/column and preview-line information.
///
/// Monty's parser stores only byte offsets per AST node to keep the post-parse
/// hot path O(1) per node. A `SourceMap` is built once per compilation, so
/// every bytecode location stores its line and column up front and a
/// suspension copies them, and once at the diagnostic boundary to resolve the
/// frames of a traceback. Building it scans the source once to index line
/// starts; with a 100k-line source this is a few hundred microseconds.
///
/// Column semantics remain exactly CPython-compatible: columns count Unicode
/// scalar values, not bytes. An ASCII source (the overwhelmingly common case
/// for Python) resolves a column in O(1); a non-ASCII source keeps a character
/// count every [`CHAR_CHECKPOINT`] bytes, so a column costs at most that many
/// bytes of counting however long the line, and compiling a wide line stays
/// linear.
pub struct SourceMap<'s> {
    source: &'s str,
    /// Byte offset of the start of each line. Length equals the number of
    /// lines; `line_starts[0]` is always 0.
    line_starts: Vec<u32>,
    /// Characters before each multiple of [`CHAR_CHECKPOINT`]; empty for an
    /// ASCII source, whose columns are byte offsets.
    char_checkpoints: Vec<usize>,
    /// Cache of preview lines, keyed by 0-based line index.
    ///
    /// Lets every `StackFrame` referencing the same source line share a
    /// single `Arc<str>` allocation rather than each cloning the line into
    /// its own `String`. This matters for deep recursion: without the
    /// cache, a 1 MiB line referenced by 1000 frames would allocate ~1 GiB;
    /// with the cache it allocates ~1 MiB. Built lazily — entries materialize
    /// only as `resolve_range` actually requests them.
    line_cache: HashMap<usize, Arc<str>>,
}

/// Bytes between the character counts a non-ASCII [`SourceMap`] records.
const CHAR_CHECKPOINT: usize = 64;

impl<'s> SourceMap<'s> {
    /// Indexes `source`: one pass for line breaks, and one for character
    /// counts when the source is not ASCII. Lookups are then O(log n).
    #[must_use]
    pub fn new(source: &'s str) -> Self {
        let bytes = source.as_bytes();
        let char_checkpoints = if source.is_ascii() {
            Vec::new()
        } else {
            let mut total = 0;
            let mut checkpoints = Vec::with_capacity(bytes.len() / CHAR_CHECKPOINT + 1);
            checkpoints.push(0);
            for chunk in bytes.as_chunks::<CHAR_CHECKPOINT>().0 {
                total += count_chars(chunk);
                checkpoints.push(total);
            }
            checkpoints
        };
        Self {
            source,
            line_starts: line_starts(bytes),
            char_checkpoints,
            line_cache: HashMap::new(),
        }
    }

    /// Number of characters in `source[..offset]`: a byte count for an ASCII
    /// source, else the nearest checkpoint plus at most one chunk of counting.
    fn chars_before(&self, offset: usize) -> usize {
        if self.char_checkpoints.is_empty() {
            offset
        } else {
            let block = offset / CHAR_CHECKPOINT;
            self.char_checkpoints[block] + count_chars(&self.source.as_bytes()[block * CHAR_CHECKPOINT..offset])
        }
    }

    /// Resolves a range's start and end (exclusive) to lines and columns, with
    /// no preview line: what a bytecode location stores at compile time.
    pub(crate) fn resolve_span(&self, range: CodeRange) -> (CodeLoc, CodeLoc) {
        (
            self.resolve_byte(range.start_byte).1,
            self.resolve_byte(range.end_byte).1,
        )
    }

    /// Resolves a `CodeRange` into `(start, end, preview_line)`.
    ///
    /// When `start` and `end` lie on the same line, `preview_line` is that
    /// single source line. The returned `Arc<str>` is shared with any other
    /// frame in this traceback resolving to the same line, so repeated
    /// lookups for the same line are O(1) and allocate only on the first
    /// lookup.
    ///
    /// When the range spans multiple lines, `preview_line` holds a
    /// pre-rendered CPython-style block (see
    /// [`multiline_preview`](Self::multiline_preview)); the renderer
    /// distinguishes the two cases by comparing `start`/`end` lines.
    pub(crate) fn resolve_range(&mut self, range: CodeRange) -> (CodeLoc, CodeLoc, Option<Arc<str>>) {
        let (start_line_idx, start) = self.resolve_byte(range.start_byte);
        let (end_line_idx, end) = self.resolve_byte(range.end_byte);
        let preview_line = if start_line_idx == end_line_idx {
            // Cache materializes lazily — first request for a given line allocates
            // the `Arc<str>`, subsequent requests for the same line clone the Arc.
            let line_text = self.line_text(start_line_idx);
            Some(Arc::clone(
                self.line_cache
                    .entry(start_line_idx)
                    .or_insert_with(|| Arc::from(line_text)),
            ))
        } else {
            // Multi-line ranges are rare (e.g. a traceback frame covering a
            // whole `class` statement), so no caching.
            Some(Arc::from(self.multiline_preview(start_line_idx, end_line_idx)))
        };
        (start, end, preview_line)
    }

    /// Renders the source preview for a range spanning several lines,
    /// mirroring CPython's traceback formatting: all lines when the range
    /// covers at most three, otherwise the first and last around a
    /// `...<N lines>...` elision marker. Displayed lines are dedented by
    /// their common leading whitespace; the caller adds the 4-space frame
    /// indent (and no caret markers — CPython omits them for these
    /// full-statement ranges).
    fn multiline_preview(&self, start_line_idx: usize, end_line_idx: usize) -> String {
        let total = end_line_idx - start_line_idx + 1;
        let displayed: Vec<&str> = if total <= 3 {
            (start_line_idx..=end_line_idx).map(|i| self.line_text(i)).collect()
        } else {
            vec![self.line_text(start_line_idx), self.line_text(end_line_idx)]
        };
        // Common leading-whitespace prefix across non-blank displayed lines,
        // comparing actual characters (not just lengths) so mixed tab/space
        // indentation never strips mismatched whitespace.
        let dedent = displayed
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| &line[..line.len() - line.trim_start().len()])
            .reduce(|a, b| common_prefix(a, b))
            .map_or(0, str::len);
        let stripped = |line: &str| line.get(dedent..).unwrap_or("").to_owned();
        if total <= 3 {
            displayed
                .iter()
                .map(|line| stripped(line))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            format!(
                "{}\n...<{} lines>...\n{}",
                stripped(displayed[0]),
                total - 2,
                stripped(displayed[1])
            )
        }
    }

    /// Resolves a raw byte offset to `(0-based line index, CodeLoc)`.
    ///
    /// Column is the number of Unicode scalar values between the line start
    /// and the offset (clamped to the source), via [`Self::chars_before`].
    fn resolve_byte(&self, byte: u32) -> (usize, CodeLoc) {
        // partition_point(|&s| s <= byte) gives the index of the first line
        // whose start is strictly greater than `byte`; subtracting one maps
        // `byte` back to the line it actually lies on.
        let line_idx = self.line_starts.partition_point(|&s| s <= byte).saturating_sub(1);
        let line_start = self.line_starts[line_idx] as usize;
        let offset = (byte as usize).min(self.source.len());
        // Ruff caps source files at 4 GiB, so any column count fits in `u32`;
        // saturate defensively if that ever changes.
        let col = u32::try_from(self.chars_before(offset) - self.chars_before(line_start)).unwrap_or(u32::MAX);
        (
            line_idx,
            CodeLoc::new(u32::try_from(line_idx).expect("line number exceeds u32"), col),
        )
    }

    /// Returns the raw text of a 0-based line index, without the trailing
    /// newline.
    fn line_text(&self, line_idx: usize) -> &'s str {
        let start = self.line_starts[line_idx] as usize;
        let end = self
            .line_starts
            .get(line_idx + 1)
            .map_or(self.source.len(), |&next| next.saturating_sub(1) as usize);
        // Guard against a trailing empty "line" past the last newline with no
        // content (e.g. when `start == source.len()`).
        let end = end.max(start);
        // `next - 1` dropped the break's last byte; a `\r\n` leaves its `\r`.
        let line = &self.source[start..end];
        line.strip_suffix('\r').unwrap_or(line)
    }
}

/// Returns the longest common prefix of `a` and `b`, always cut on a char
/// boundary. Used by [`SourceMap::multiline_preview`] to find the shared
/// indentation of the displayed lines.
fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let end = a
        .char_indices()
        .zip(b.chars())
        .find(|&((_, ca), cb)| ca != cb)
        // All zipped chars equal: the shorter string is the common prefix, and
        // equal chars encode identically so its byte length indexes `a` safely.
        .map_or(a.len().min(b.len()), |((i, _), _)| i);
    &a[..end]
}

/// Crate-internal builders for [`StackFrame`] (which lives in `monty-types`):
/// they resolve interned names and raw byte offsets via [`Interns`] /
/// [`SourceMap`], which only exist interpreter-side.
pub(crate) trait StackFrameExt {
    /// Builds a runtime `StackFrame` from an internal `RawStackFrame`.
    ///
    /// Resolves the raw filename/frame-name `StringId`s via `interns` and
    /// expands the position's byte offsets to line/column and a preview
    /// line via `source_map`.
    fn from_raw(f: &RawStackFrame, interns: &Interns, source_map: &mut SourceMap<'_>) -> StackFrame {
        let filename = interns.get_filename(f.position.filename).to_string();
        let (start, end, preview_line) = source_map.resolve_range(f.position);
        StackFrame {
            filename,
            start,
            end,
            frame_name: f.frame_name.map(|id| interns.get_str(id).to_string()),
            preview_line,
            hide_caret: f.hide_caret,
            hide_frame_name: false,
        }
    }

    /// Builds a `StackFrame` for a `SyntaxError`.
    ///
    /// Sets `hide_frame_name: true` because CPython's SyntaxError format
    /// omits the trailing `, in <module>` part.
    fn from_position_syntax_error(position: CodeRange, filename: &str, source_map: &mut SourceMap<'_>) -> StackFrame {
        let (start, end, preview_line) = source_map.resolve_range(position);
        StackFrame {
            filename: filename.to_string(),
            start,
            end,
            frame_name: None,
            preview_line,
            hide_caret: false,
            hide_frame_name: true,
        }
    }

    /// Builds a generic `StackFrame` from a `CodeRange` and filename.
    ///
    /// Used for runtime-style errors raised outside the VM's frame tracking
    /// (e.g. parse-phase `NotImplementedError`) where caret markers and the
    /// `, in <module>` suffix are both shown.
    fn from_position(position: CodeRange, filename: &str, source_map: &mut SourceMap<'_>) -> StackFrame {
        let (start, end, preview_line) = source_map.resolve_range(position);
        StackFrame {
            filename: filename.to_string(),
            start,
            end,
            frame_name: None,
            preview_line,
            hide_caret: false,
            hide_frame_name: false,
        }
    }

    /// Builds a `StackFrame` with caret markers suppressed.
    ///
    /// Used for errors like `ImportError` and `ModuleNotFoundError`, where
    /// CPython shows the source preview line but no `~~~` carets beneath it.
    fn from_position_no_caret(position: CodeRange, filename: &str, source_map: &mut SourceMap<'_>) -> StackFrame {
        let (start, end, preview_line) = source_map.resolve_range(position);
        StackFrame {
            filename: filename.to_string(),
            start,
            end,
            frame_name: None,
            preview_line,
            hide_caret: true,
            hide_frame_name: false,
        }
    }
}

impl StackFrameExt for StackFrame {}
