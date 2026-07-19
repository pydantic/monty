//! Length-prefixed framing for protocol messages over byte streams.
//!
//! Each frame is a 4-byte unsigned **little-endian** length prefix followed by
//! that many bytes of protobuf. LE matches the convention used elsewhere in
//! monty's serialization and is trivial to implement in any language
//! (`readUInt32LE` in Node, `struct.unpack('<I', ...)` in Python).
//!
//! The reader enforces [`MAX_FRAME_LEN`] so a corrupted or byzantine peer
//! cannot make the receiving process allocate unbounded memory from a single
//! bogus length prefix. Writers flush after every frame — the protocol is a
//! strict alternation, so an unflushed frame would deadlock both sides.

use std::{
    error, fmt,
    io::{self, Read, Write},
};

use prost::Message;

use crate::wire::reset_decode_budget;

/// Default maximum frame length (256 MiB).
///
/// Far above any sane payload, but small enough that a corrupted length
/// prefix cannot trigger a multi-gigabyte allocation in the receiver.
pub const MAX_FRAME_LEN: u32 = 256 * 1024 * 1024;

/// Hard, fixed per-frame budget for *resident* decoded value bytes (1 GiB = 4×
/// the frame cap).
///
/// `MAX_FRAME_LEN` bounds the *wire* size, but the cheapest elements (`None` in a
/// list ≈ 4 wire bytes) decode into 88-byte `MontyObject`s — a ~22× blow-up that
/// could turn a ≤256 MiB frame into multiple GiB on the host. The budget caps
/// decoded size so amplification is bounded regardless of frame contents.
///
/// The budget bounds bytes *resident* at once. The decoder materializes every
/// payload straight into its final type — containers via `ObjectList`/
/// `PairList`/`NamedTupleBody`/`DataclassBody`, and function-call args &
/// kwargs via `WireFunctionCall` — so no path builds an
/// intermediate `Vec<WireObject>`/`Vec<Pair>` and then converts it; only a
/// single per-element value is transient at any moment. The host *peak* is
/// therefore ~1× the budget plus the ≤256 MiB frame buffer (~1.25 GiB); the 4×
/// multiplier keeps the hard 1 GiB ceiling comfortably below host limits.
/// Multiplies per concurrent worker.
pub const DEFAULT_MAX_DECODE_BYTES: usize = 4 * MAX_FRAME_LEN as usize;

/// Framing or decoding failure while reading or writing protocol messages.
#[derive(Debug)]
pub enum FrameError {
    /// Underlying stream I/O failure (includes broken pipes — peer death).
    Io(io::Error),
    /// Frame contents were not a valid protobuf message.
    Decode(prost::DecodeError),
    /// Length prefix exceeded the reader's maximum frame length.
    FrameTooLarge {
        /// Length claimed by the prefix.
        len: u32,
        /// The reader's configured maximum.
        max: u32,
    },
    /// The stream ended mid-frame: the peer died while writing.
    Truncated,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "frame I/O error: {e}"),
            Self::Decode(e) => write!(f, "frame decode error: {e}"),
            Self::FrameTooLarge { len, max } => write!(f, "frame of {len} bytes exceeds maximum of {max} bytes"),
            Self::Truncated => f.write_str("stream ended mid-frame"),
        }
    }
}

impl error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Encodes `msg` and writes it to `writer` as one length-prefixed frame, then
/// flushes (see the module docs for why flushing every frame is required).
///
/// Frames above [`MAX_FRAME_LEN`] fail with [`FrameError::FrameTooLarge`]
/// *before* anything is written, keeping the stream in sync so the caller
/// can degrade gracefully instead of desynchronizing the protocol.
pub fn write_frame(writer: &mut impl Write, msg: &impl Message) -> Result<(), FrameError> {
    let body = encode_to_capped_vec(msg)?;
    let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

/// Encodes `msg` to a `Vec<u8>`, enforcing [`MAX_FRAME_LEN`] *before* encoding
/// (so a >256 MiB message is rejected without allocating it).
///
/// Message-oriented transports (e.g. a WebSocket, where the message boundary is
/// the frame) use this directly instead of [`write_frame`]: they send the bytes
/// with no length prefix but still need the same oversize guard so the wire size
/// cap is identical across transports.
pub fn encode_to_capped_vec(msg: &impl Message) -> Result<Vec<u8>, FrameError> {
    // Size before encoding to avoid building a giant Vec just to reject it.
    let encoded_len = msg.encoded_len();
    u32::try_from(encoded_len)
        .ok()
        .filter(|&len| len <= MAX_FRAME_LEN)
        .ok_or(FrameError::FrameTooLarge {
            len: u32::try_from(encoded_len).unwrap_or(u32::MAX),
            max: MAX_FRAME_LEN,
        })?;
    // encode_to_vec cannot fail (Vec<u8> grows as needed)
    Ok(msg.encode_to_vec())
}

/// Decodes one already-deframed message from `bytes`.
///
/// The message-oriented counterpart to one [`FrameReader::read`]: a transport
/// whose boundary *is* the frame (a WebSocket) hands the payload straight here.
/// Resets the per-frame decode budget first so an untrusted peer gets the same
/// host-memory bound as the length-prefixed reader, and rejects payloads over
/// [`MAX_FRAME_LEN`].
pub fn decode_frame<M: Message + Default>(bytes: &[u8]) -> Result<M, FrameError> {
    if bytes.len() > MAX_FRAME_LEN as usize {
        return Err(FrameError::FrameTooLarge {
            len: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            max: MAX_FRAME_LEN,
        });
    }
    reset_decode_budget();
    M::decode(bytes).map_err(FrameError::Decode)
}

/// Reads length-prefixed protobuf frames from a byte stream.
#[derive(Debug)]
pub struct FrameReader<R: Read> {
    inner: R,
    max_frame_len: u32,
}

impl<R: Read> FrameReader<R> {
    /// Wraps a byte stream with the default [`MAX_FRAME_LEN`].
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            max_frame_len: MAX_FRAME_LEN,
        }
    }

    /// Wraps a byte stream with a custom maximum frame length.
    pub fn with_max_frame_len(inner: R, max_frame_len: u32) -> Self {
        Self { inner, max_frame_len }
    }

    /// Reads one frame and decodes it as `M`.
    ///
    /// Returns `Ok(None)` on a clean EOF at a frame boundary (the peer closed
    /// the stream between messages). EOF *inside* a frame is
    /// [`FrameError::Truncated`] — the peer died mid-write.
    pub fn read<M: Message + Default>(&mut self) -> Result<Option<M>, FrameError> {
        let mut len_bytes = [0u8; 4];
        match read_exact_or_eof(&mut self.inner, &mut len_bytes)? {
            ReadOutcome::CleanEof => return Ok(None),
            ReadOutcome::Truncated => return Err(FrameError::Truncated),
            ReadOutcome::Filled => {}
        }
        let len = u32::from_le_bytes(len_bytes);
        if len > self.max_frame_len {
            return Err(FrameError::FrameTooLarge {
                len,
                max: self.max_frame_len,
            });
        }
        // Allocation is up front but bounded by `max_frame_len` (256 MiB by
        // default). A streaming protobuf decoder would add complexity, while
        // this keeps byzantine peers bounded to one frame buffer per blocked
        // reader.
        let mut body = vec![0u8; len as usize];
        match read_exact_or_eof(&mut self.inner, &mut body)? {
            ReadOutcome::Filled => {}
            // EOF after a length prefix is always mid-frame.
            ReadOutcome::CleanEof | ReadOutcome::Truncated => return Err(FrameError::Truncated),
        }
        // Bound host memory for this decode: the wire size is capped, but cheap
        // elements amplify ~22× into `MontyObject`s. Reset the per-frame budget.
        reset_decode_budget();
        M::decode(body.as_slice()).map(Some).map_err(FrameError::Decode)
    }
}

/// Outcome of [`read_exact_or_eof`].
enum ReadOutcome {
    /// The buffer was completely filled.
    Filled,
    /// EOF before any byte was read.
    CleanEof,
    /// EOF after some but not all bytes were read.
    Truncated,
}

/// Like `read_exact` but distinguishes "EOF at the boundary" from "EOF
/// mid-buffer", which the framing layer must report differently.
fn read_exact_or_eof(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<ReadOutcome> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                return Ok(if filled == 0 {
                    ReadOutcome::CleanEof
                } else {
                    ReadOutcome::Truncated
                });
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(ReadOutcome::Filled)
}
