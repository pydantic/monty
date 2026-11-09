//! The byte budget on streamed calls' arguments.
//!
//! A streamed call reaches the host without suspending the worker, so the
//! suspension count alone lets sandbox code make the host hold
//! `max_suspensions` large argument arenas at once while the worker stays
//! small. This caps their total size from arrival until the host sends a call's
//! result or a new feed starts.

use std::collections::HashMap;

use monty_proto::DEFAULT_MAX_DECODE_BYTES;

/// Argument bytes of the streamed calls whose results the host has not yet sent.
///
/// Sizes are wire bytes: each `StreamedCalls` frame's length split evenly
/// across its calls, since the frame is the unit the parent decodes.
#[derive(Debug)]
pub(crate) struct StreamedArgs {
    /// The most bytes that may be held at once.
    limit: usize,
    /// Bytes charged per call, released by [`Self::release`].
    by_call: HashMap<u32, usize>,
    /// Sum of `by_call`.
    held: usize,
}

impl StreamedArgs {
    /// A budget of `limit` bytes; see [`streamed_args_limit`].
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            by_call: HashMap::new(),
            held: 0,
        }
    }

    /// The configured ceiling, for error messages.
    pub(crate) const fn limit(&self) -> usize {
        self.limit
    }

    /// Charges one frame of `frame_len` bytes carrying the non-empty `call_ids`;
    /// `false` once the total would pass the limit, leaving the budget unchanged.
    pub(crate) fn charge(&mut self, frame_len: usize, call_ids: &[u32]) -> bool {
        debug_assert!(!call_ids.is_empty(), "a frame with no kept calls holds nothing");
        let Some(held) = self.held.checked_add(frame_len).filter(|&held| held <= self.limit) else {
            return false;
        };
        self.held = held;
        let count = call_ids.len().max(1);
        let share = frame_len / count;
        for (i, &call_id) in call_ids.iter().enumerate() {
            // the first call takes the remainder, so the shares sum to the frame
            let bytes = if i == 0 { share + frame_len % count } else { share };
            *self.by_call.entry(call_id).or_default() += bytes;
        }
        true
    }

    /// Releases the bytes charged for `call_id`, once its result is sent.
    pub(crate) fn release(&mut self, call_id: u32) {
        if let Some(bytes) = self.by_call.remove(&call_id) {
            self.held -= bytes;
        }
    }

    /// Releases everything, when a new feed starts or the session is replaced.
    pub(crate) fn clear(&mut self) {
        self.by_call.clear();
        self.held = 0;
    }
}

/// The budget a pool with `max_decode_bytes` gives each session: a quarter of
/// it, so 256 MiB by default, the size of one maximum frame. Decoding never
/// uses more than `DEFAULT_MAX_DECODE_BYTES`, so neither does this.
pub(crate) fn streamed_args_limit(max_decode_bytes: usize) -> usize {
    max_decode_bytes.min(DEFAULT_MAX_DECODE_BYTES) / 4
}
