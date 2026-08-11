use std::{collections::HashMap, sync::Arc};

use monty_types::{CodeLoc, StackFrame};

use crate::{exception_private::RawStackFrame, intern::Interns, parse::CodeRange};

/// Lazy resolver from raw byte offsets (stored on every [`CodeRange`]) back to
/// human-readable line/column/preview-line information.
///
/// Monty's parser stores only byte offsets per AST node to keep the post-parse
/// hot path O(1) per node. `SourceMap` is built once at the diagnostic
/// boundary — when converting an internal error into a public
/// [`MontyException`] — and used to resolve every frame in the traceback.
/// Building it scans the source once to index line starts; with a 100k-line
/// source this is a few hundred microseconds and fires only when an exception
/// is actually raised.
///
/// Column semantics remain exactly CPython-compatible: columns count Unicode
/// scalar values, not bytes. The ASCII fast path (the overwhelmingly common
/// case for Python source) skips the `chars()` iterator entirely.
pub struct SourceMap<'s> {
    source: &'s str,
    /// Byte offset of the start of each line. Length equals the number of
    /// lines; `line_starts[0]` is always 0.
    line_starts: Vec<u32>,
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

impl<'s> SourceMap<'s> {
    /// Builds a line-start index over `source`.
    ///
    /// Amortizes across every frame in the traceback — one O(n) scan, then
    /// O(log n) lookups per frame.
    #[must_use]
    pub fn new(source: &'s str) -> Self {
        let mut line_starts = Vec::with_capacity(source.len() / 40 + 1);
        line_starts.push(0);
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                // source should never exceed 4 GB
                let start = u32::try_from(i + 1).unwrap_or(u32::MAX);
                line_starts.push(start);
            }
        }
        Self {
            source,
            line_starts,
            line_cache: HashMap::new(),
        }
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
    /// and the offset; uses an ASCII fast path when the preceding slice is
    /// pure ASCII.
    fn resolve_byte(&self, byte: u32) -> (usize, CodeLoc) {
        // partition_point(|&s| s <= byte) gives the index of the first line
        // whose start is strictly greater than `byte`; subtracting one maps
        // `byte` back to the line it actually lies on.
        let line_idx = self.line_starts.partition_point(|&s| s <= byte).saturating_sub(1);
        let line_start = self.line_starts[line_idx];
        let slice_start = line_start as usize;
        let slice_end = (byte as usize).min(self.source.len());
        let slice = &self.source[slice_start..slice_end];
        // Ruff caps source files at 4 GiB, so any byte-based column count fits
        // comfortably in `u32`; saturate defensively if that ever changes.
        let col = if slice.is_ascii() {
            u32::try_from(slice.len()).unwrap_or(u32::MAX)
        } else {
            u32::try_from(slice.chars().count()).unwrap_or(u32::MAX)
        };
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
        // Strip a trailing `\r` if the source uses CRLF line endings.
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
        let filename = interns.get_str(f.position.filename).to_string();
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
